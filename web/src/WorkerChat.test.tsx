import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import WorkerChat, { clearWorkerChatRetention } from './WorkerChat'
import type { RouteState } from './client-contract'
import { actorIdentityKey } from './protocol'
import { HistoryReadError, readHistoryPage, type HistoryPage } from './history-client'
import type { HarnessViewModel } from './view-model'
vi.mock('./history-client', async original => ({ ...await original<typeof import('./history-client')>(), readHistoryPage: vi.fn() }))
const identity = { run: 'run', actor: '/root/worker', incarnation: 'one' }
const actor = { id: actorIdentityKey(identity), name: identity.actor, run: identity.run, incarnation: identity.incarnation,
  kind: 'model' as const, lifecycle: 'running', modelConversation: 'conversation' }
const route: RouteState = { screen: 'chat', selection: { kind: 'actor', identity }, global: false, messageFilters: {} }
const data: HarnessViewModel = { hostRun: 'run', actors: [actor], nodes: [{ id: 'conversation', name: '/root/worker', state: 'active' }],
  timeline: [{ id: 'head', nodeId: 'conversation', kind: 'request', state: 'completed', label: 'Head', parentId: null, historyRefreshKey: 'head:1' }], inbox: [] }
function history(id: string, text = `Messages for ${id}`): HistoryPage {
  return { requestId: id, parentId: null, branch: '/root/worker', nextOffset: null, oversizedItem: null,
    items: [{ position: '0', hash: 'a'.repeat(64), byteLen: '100', item: { type: 'message', role: 'assistant', content: text } }] }
}
const navigate = vi.fn()
function chat(snapshot = data, selected = route, phase: 'ready' | 'disconnected' = 'ready') {
  return <WorkerChat data={snapshot} route={selected} navigate={navigate} transportPhase={phase} />
}
beforeEach(() => {
  clearWorkerChatRetention(); vi.clearAllMocks()
  window.history.replaceState(null, '', '/')
  vi.mocked(readHistoryPage).mockImplementation(async id => history(id))
})
describe('exact worker Chat', () => {
  it.each(['retired', 'lost'])('loads durable history for an initially %s worker and after reload', async lifecycle => {
    const retired = { ...data, actors: [{ ...actor, lifecycle }] }
    const first = render(chat(retired))
    await screen.findByText('Messages for head')
    expect(screen.getByText(new RegExp(`This actor is ${lifecycle}`))).toHaveTextContent('read-only')
    expect(screen.getByRole('button', { name: 'Refresh messages' })).toBeEnabled()
    first.unmount(); clearWorkerChatRetention()
    render(chat(retired))
    await screen.findByText('Messages for head')
    expect(readHistoryPage).toHaveBeenCalledTimes(2)
  })
  it('retains exact history through retirement, another worker, and a missing old incarnation', async () => {
    const otherIdentity = { ...identity, actor: '/root/other', incarnation: 'other' }
    const other = { ...actor, id: actorIdentityKey(otherIdentity), name: otherIdentity.actor, incarnation: otherIdentity.incarnation, modelConversation: 'other' }
    const both = { ...data, actors: [actor, other], timeline: [...data.timeline, { ...data.timeline[0]!, id: 'other-head', nodeId: 'other' }] }
    const mounted = render(chat(both))
    await screen.findByText('Messages for head')
    mounted.rerender(chat({ ...both, actors: [{ ...actor, lifecycle: 'retired' }, other] }))
    const otherRoute = { ...route, selection: { kind: 'actor' as const, identity: otherIdentity } }
    mounted.rerender(chat(both, otherRoute))
    await screen.findByText('Messages for other-head')
    expect(screen.queryByText('Messages for head')).toBeNull()
    mounted.rerender(chat({ ...both, actors: [other], timeline: [] }, route, 'disconnected'))
    expect(screen.getByText('Messages for head')).toBeVisible()
    expect(screen.queryByText('Messages for other-head')).toBeNull()
    expect(screen.getByText(/This exact actor is unavailable/)).toBeVisible()
  })
  it('preserves visited history after navigating to Tree and back while disconnected', async () => {
    const mounted = render(chat())
    await screen.findByText('Messages for head')
    mounted.unmount()
    render(chat({ ...data, actors: [], timeline: [] }, route, 'disconnected'))
    expect(screen.getByText('Messages for head')).toBeVisible()
    expect(readHistoryPage).toHaveBeenCalledTimes(1)
  })
  it('never infers a retired actor association from a replacement or conversation path', async () => {
    const replacement = { ...actor, id: actorIdentityKey({ ...identity, incarnation: 'two' }), incarnation: 'two' }
    render(chat({ ...data, actors: [replacement] }))
    expect(screen.getByText(/no retained conversation association/)).toBeVisible()
    expect(readHistoryPage).not.toHaveBeenCalled()
    expect(navigate).not.toHaveBeenCalled()
    const replacementLink = screen.getByRole('link', { name: '/root/worker', current: false })
    expect(new URL(replacementLink.getAttribute('href')!).searchParams.get('incarnation')).toBe('two')
    const oldLink = screen.getByRole('link', { name: '/root/worker', current: 'page' })
    expect(new URL(oldLink.getAttribute('href')!).searchParams.get('incarnation')).toBe('one')
  })
  it.each(['missing', 'lost association'])('keeps the exact old head when its %s and a replacement reuses its conversation', async availability => {
    const mounted = render(chat())
    await screen.findByText('Messages for head')
    const replacementIdentity = { ...identity, incarnation: 'two' }
    const replacement = { ...actor, id: actorIdentityKey(replacementIdentity), incarnation: 'two' }
    const old = { ...actor, lifecycle: 'lost', modelConversation: undefined }
    mounted.rerender(chat({ ...data, actors: availability === 'missing' ? [replacement] : [old, replacement],
      timeline: [...data.timeline, { ...data.timeline[0]!, id: 'replacement-head', parentId: 'head' }] }))
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledTimes(2))
    expect(screen.getByText('Messages for head')).toBeVisible()
    expect(screen.queryByText('Messages for replacement-head')).toBeNull()
    expect(vi.mocked(readHistoryPage).mock.calls.every(call => call[0] === 'head')).toBe(true)
  })
  it('keeps the known retired head when both old and replacement identities remain projected on the same conversation', async () => {
    const mounted = render(chat())
    await screen.findByText('Messages for head')
    const replacementIdentity = { ...identity, incarnation: 'two' }
    const replacement = { ...actor, id: actorIdentityKey(replacementIdentity), incarnation: 'two' }
    const shared = { ...data, actors: [{ ...actor, lifecycle: 'retired' }, replacement],
      timeline: [...data.timeline, { ...data.timeline[0]!, id: 'replacement-head', parentId: 'head' }] }
    mounted.rerender(chat(shared))
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledTimes(2))
    expect(screen.getByText('Messages for head')).toBeVisible()
    expect(screen.getByText(/multiple exact actors/)).toBeVisible()
    expect(vi.mocked(readHistoryPage).mock.calls.every(call => call[0] === 'head')).toBe(true)
    mounted.rerender(chat(shared, { ...route, selection: { kind: 'actor', identity: replacementIdentity } }))
    expect(screen.getByText('No exact history head is available for this actor.')).toBeVisible()
    expect(screen.queryByText('Messages for head')).toBeNull()
    expect(screen.queryByText('Messages for replacement-head')).toBeNull()
    expect(readHistoryPage).toHaveBeenCalledTimes(2)
  })
  it.each(['retired', 'running'])('does not guess a fresh %s actor head when exact conversation associations are ambiguous', lifecycle => {
    const replacementIdentity = { ...identity, incarnation: 'two' }
    const replacement = { ...actor, id: actorIdentityKey(replacementIdentity), incarnation: 'two', lifecycle }
    const shared = { ...data, actors: [{ ...actor, lifecycle: 'retired' }, replacement],
      timeline: [...data.timeline, { ...data.timeline[0]!, id: 'replacement-head', parentId: 'head' }] }
    render(chat(shared, lifecycle === 'retired' ? route : { ...route, selection: { kind: 'actor', identity: replacementIdentity } }))
    expect(screen.getByText(/multiple exact actors/)).toBeVisible()
    expect(screen.getByText('No exact history head is available for this actor.')).toBeVisible()
    expect(readHistoryPage).not.toHaveBeenCalled()
  })
  it('opens a fresh retired exact head outside the activity limit and isolates the replacement head', async () => {
    const replacementIdentity = { ...identity, incarnation: 'two' }
    const shared = { ...data, actors: [{ ...actor, lifecycle: 'retired', modelHeadRequest: 'retired-head' },
      { ...actor, id: actorIdentityKey(replacementIdentity), incarnation: 'two', modelHeadRequest: 'replacement-head' }],
      timeline: Array.from({ length: 128 }, (_, index) => ({ ...data.timeline[0]!, id: index === 127 ? 'replacement-head' : `sibling-${index}` })) }
    const mounted = render(chat(shared))
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledWith('retired-head', '0', expect.any(AbortSignal)))
    expect(vi.mocked(readHistoryPage).mock.calls.map(call => call[0])).toEqual(['retired-head'])
    mounted.rerender(chat(shared, { ...route, selection: { kind: 'actor', identity: replacementIdentity } }))
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledWith('replacement-head', '0', expect.any(AbortSignal)))
    expect(vi.mocked(readHistoryPage).mock.calls.map(call => call[0])).toEqual(['retired-head', 'replacement-head'])
  })
  it('loads only the selected conversation and uses ordinary exact worker anchors', async () => {
    const childIdentity = { ...identity, actor: '/root/child', incarnation: 'child' }
    const child = { ...actor, id: actorIdentityKey(childIdentity), name: childIdentity.actor, incarnation: childIdentity.incarnation, modelConversation: 'child' }
    render(chat({ ...data, actors: [actor, child], timeline: [...data.timeline, { ...data.timeline[0]!, id: 'child-head', nodeId: 'child' }] }))
    await screen.findByText('Messages for head')
    expect(readHistoryPage).toHaveBeenCalledTimes(1)
    const link = screen.getByRole('link', { name: '/root/child' })
    const url = new URL(link.getAttribute('href')!)
    expect(url.searchParams.get('run')).toBe('run'); expect(url.searchParams.get('incarnation')).toBe('child')
    // Cancel jsdom's navigation only after observing React's modifier handling.
    let modifiedPrevented = true
    document.addEventListener('click', event => { modifiedPrevented = event.defaultPrevented; event.preventDefault() }, { once: true })
    fireEvent.click(link, { ctrlKey: true })
    expect(modifiedPrevented).toBe(false); expect(navigate).not.toHaveBeenCalled()
    fireEvent.click(link); expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ selection: { kind: 'actor', identity: childIdentity } }))
  })
  it('does not fetch workflow actors, unselected Chats or invalid routes and preserves composer children', () => {
    const mounted = render(<WorkerChat data={{ ...data, actors: [{ ...actor, kind: 'workflow' }] }} route={route} navigate={navigate} transportPhase="ready"><p>Composer from App</p></WorkerChat>)
    expect(screen.getByText(/Workflow actors have no model exchange/)).toBeVisible()
    expect(screen.getByText('Composer from App')).toBeVisible()
    mounted.rerender(chat(data, { ...route, selection: { kind: 'none' } }))
    expect(screen.getByText(/Select a worker/)).toBeVisible()
    mounted.rerender(<WorkerChat data={data} route={route} navigate={navigate} transportPhase="ready" issue="Malformed exact link" />)
    expect(screen.getByRole('alert')).toHaveTextContent('Malformed exact link')
    expect(readHistoryPage).not.toHaveBeenCalled()
  })
  it('aborts an old selected read, ignores its late result, and reconnects to the exact new head', async () => {
    let resolveOld: ((page: HistoryPage) => void) | undefined
    vi.mocked(readHistoryPage).mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve }))
    const mounted = render(chat())
    await waitFor(() => expect(readHistoryPage).toHaveBeenCalledTimes(1))
    const signal = vi.mocked(readHistoryPage).mock.calls[0]![2]
    const next = { ...data, timeline: [{ ...data.timeline[0]!, id: 'next' }] }
    mounted.rerender(chat(next, route, 'disconnected'))
    expect(signal.aborted).toBe(true)
    await act(async () => { resolveOld?.(history('head', 'Stale response')) })
    expect(screen.queryByText('Stale response')).toBeNull()
    mounted.rerender(chat(next))
    await screen.findByText('Messages for next')
    expect(readHistoryPage).toHaveBeenLastCalledWith('next', '0', expect.any(AbortSignal))
  })
  it('keeps fetched retired history when refresh fails and retries protected history', async () => {
    const expired = vi.fn()
    render(<WorkerChat data={{ ...data, actors: [{ ...actor, lifecycle: 'retired' }] }} route={route} navigate={navigate} transportPhase="ready" onAuthExpired={expired} />)
    await screen.findByText('Messages for head')
    vi.mocked(readHistoryPage).mockRejectedValueOnce(new HistoryReadError('Sign in again', 'authentication'))
    fireEvent.click(screen.getByRole('button', { name: 'Refresh messages' }))
    await screen.findByText('Sign in again')
    expect(screen.getByText('Messages for head')).toBeVisible(); expect(expired).toHaveBeenCalledOnce()
    fireEvent.click(screen.getByRole('button', { name: 'Retry messages' }))
    await waitFor(() => expect(screen.queryByText('Sign in again')).toBeNull())
  })
  it('bounds the worker list and exposes retired workers without requesting their histories', () => {
    const actors = Array.from({ length: 205 }, (_, index) => ({ ...actor, id: String(index), name: `/worker/${index}`, incarnation: String(index), lifecycle: index < 100 ? 'retired' : 'running' }))
    render(chat({ ...data, actors }, { ...route, selection: { kind: 'none' } }))
    const list = screen.getByRole('complementary', { name: 'Workers' })
    expect(within(list).getAllByRole('link')).toHaveLength(100)
    fireEvent.click(screen.getByRole('button', { name: 'Next workers' }))
    expect(within(list).getAllByRole('link')).toHaveLength(100)
    fireEvent.change(screen.getByRole('searchbox', { name: 'Find worker' }), { target: { value: 'retired' } })
    expect(within(list).getAllByRole('link')).toHaveLength(100)
    expect(readHistoryPage).not.toHaveBeenCalled()
  })
  it('searches displayed run values and jumps to the selected exact worker page', () => {
    const actors = Array.from({ length: 205 }, (_, index) => {
      const identity = { run: `run-${index}`, actor: `/worker/${String(index).padStart(3, '0')}`, incarnation: String(index) }
      return { ...actor, id: actorIdentityKey(identity), name: identity.actor, run: identity.run, incarnation: identity.incarnation, lifecycle: 'running' }
    })
    const selected = actors[150]!
    const selectedIdentity = { run: selected.run, actor: selected.name, incarnation: selected.incarnation }
    const selectedRoute = { ...route, selection: { kind: 'actor' as const, identity: selectedIdentity } }
    render(chat({ ...data, actors }, selectedRoute))

    const list = screen.getByRole('complementary', { name: 'Workers' })
    fireEvent.change(screen.getByRole('searchbox', { name: 'Find worker' }), { target: { value: 'run-150' } })
    expect(within(list).getByRole('link', { name: selected.name })).toBeVisible()
    expect(within(list).queryByRole('link', { name: actors[0]!.name })).toBeNull()

    fireEvent.change(screen.getByRole('searchbox', { name: 'Find worker' }), { target: { value: 'no match' } })
    expect(within(list).getByText('No workers match.')).toBeVisible()
    fireEvent.click(within(list).getByRole('button', { name: 'Jump to selected worker' }))
    expect(screen.getByRole('searchbox', { name: 'Find worker' })).toHaveValue('')
    expect(within(list).getByText('Page 2 of 3')).toBeVisible()
    const selectedLink = within(list).getByRole('link', { name: selected.name })
    expect(selectedLink).toHaveAttribute('aria-current', 'page')
    expect(navigate).not.toHaveBeenCalled()
    expect(readHistoryPage).not.toHaveBeenCalled()
  })
  it('keeps search, page, and URL unchanged when the selected exact worker is unavailable', () => {
    const actors = Array.from({ length: 205 }, (_, index) => {
      const identity = { run: `run-${index}`, actor: `/worker/${String(index).padStart(3, '0')}`, incarnation: String(index) }
      return { ...actor, id: actorIdentityKey(identity), name: identity.actor, run: identity.run, incarnation: identity.incarnation, lifecycle: 'running' }
    })
    const unavailableRoute = { ...route, selection: { kind: 'actor' as const, identity: { run: 'old-run', actor: '/worker/old', incarnation: 'old-incarnation' } } }
    window.history.replaceState(null, '', '/chat/worker/old?run=old-run&incarnation=old-incarnation')
    const url = window.location.href
    render(chat({ ...data, actors }, unavailableRoute))

    const list = screen.getByRole('complementary', { name: 'Workers' })
    const search = screen.getByRole('searchbox', { name: 'Find worker' })
    fireEvent.change(search, { target: { value: 'run-' } })
    fireEvent.click(within(list).getByRole('button', { name: 'Next workers' }))
    expect(search).toHaveValue('run-')
    expect(within(list).getByText('Page 2 of 3')).toBeVisible()
    expect(within(list).queryByRole('button', { name: 'Jump to selected worker' })).toBeNull()
    expect(window.location.href).toBe(url)
    expect(navigate).not.toHaveBeenCalled()
  })
  it('clears retained messages and actor associations on deliberate signout', async () => {
    const mounted = render(chat())
    await screen.findByText('Messages for head'); mounted.unmount()
    clearWorkerChatRetention()
    render(chat({ ...data, actors: [], timeline: [] }, route, 'disconnected'))
    expect(screen.queryByText('Messages for head')).toBeNull()
    expect(screen.getByText(/no retained conversation association/)).toBeVisible()
  })
})
