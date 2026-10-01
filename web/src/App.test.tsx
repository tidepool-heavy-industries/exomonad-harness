import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import App from './App'
import type { HarnessViewModel } from './view-model'

const emptyData: HarnessViewModel = { nodes: [], timeline: [], inbox: [] }

describe('operator views', () => {
  it('teaches the empty inbox and navigates between views by keyboard', () => {
    render(<App data={emptyData} />)
    fireEvent.click(screen.getByRole('button', { name: 'Inbox' }))
    expect(screen.getByText('Inbox is clear')).toBeInTheDocument()
    fireEvent.keyDown(window, { key: 'g' })
    fireEvent.keyDown(window, { key: 't' })
    expect(screen.getByRole('heading', { name: 'Tree' })).toBeInTheDocument()
  })

  it('renders protocol-adapted records and passes commands without inventing a result', () => {
    const data: HarnessViewModel = {
      nodes: [{ id: 'n-1', name: '/root', state: 'waiting on operator', model: 'gpt-6-luna' }],
      timeline: [{ id: 'e-1', nodeId: 'n-1', label: 'spawn_agent', kind: 'job', state: 'running' }],
      inbox: [{ id: 'm-1', sender: '/root', message: 'Continue?', state: 'unread' }],
    }
    const onCommand = vi.fn()
    render(<App data={data} onCommand={onCommand} />)
    expect(screen.getByText('/root')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }))
    expect(screen.getByText(/spawn_agent/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Inbox' }))
    expect(screen.getByText('Continue?')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Command' }))
    fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'wait_agent' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(onCommand).toHaveBeenCalledWith('wait_agent')
    expect(screen.queryByText('Command sent')).not.toBeInTheDocument()
  })

  it('offers deterministic actions and preserves payload whitespace after the verb', () => {
    const onCommand = vi.fn()
    render(<App data={emptyData} onCommand={onCommand} />)
    fireEvent.click(screen.getByRole('button', { name: 'Command' }))
    expect(screen.getByText(/Deterministic mode/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel wait' }))
    expect(onCommand).toHaveBeenCalledWith('cancel')
    fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'echo  keep spaces' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(onCommand).toHaveBeenLastCalledWith('echo  keep spaces')
    expect(screen.getByText(/Sending is not acceptance or completion/)).toBeInTheDocument()
    expect(screen.getByText(/Outcomes, progress, and messages appear only to the extent represented/)).toBeInTheDocument()
  })

  it('presents server command outcomes and real ordered message endpoints', () => {
    const data: HarnessViewModel = {
      nodes: [{ id: 'root', name: '/root', state: 'idle' }],
      timeline: [{
        id: 'req', nodeId: 'root', label: 'echo hello', kind: 'request', state: 'completed',
        commandId: 'cmd-7', command: 'echo hello', outcome: 'completed', detail: 'echoed hello',
      }],
      inbox: [{
        id: 'progress', sender: '/harness', recipient: '/root', message: 'working',
        state: 'PROGRESS', ordinal: 1,
      }],
    }
    render(<App data={data} />)
    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }))
    expect(screen.getByText(/command cmd-7/)).toBeInTheDocument()
    expect(screen.getByText(/outcome completed/)).toBeInTheDocument()
    expect(screen.getByText(/echoed hello/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Inbox' }))
    expect(screen.getByText('/harness → /root')).toBeInTheDocument()
    expect(screen.getByText('PROGRESS · #1')).toBeInTheDocument()
    expect(screen.getByText('working')).toBeInTheDocument()
  })

  it('shows authoritative async job identity, lifecycle, delivery, and output', () => {
    const data: HarnessViewModel = {
      nodes: [{ id: 'root', name: '/root', state: 'paused' }],
      timeline: [{
        id: 'job-9', nodeId: 'root', label: 'Async job', kind: 'job', state: 'settled',
        requestId: 'request-1', callId: 'call-3', toolKind: 'function', toolName: 'slow_tool',
        delivered: false, output: 'result pending delivery',
      }],
      inbox: [],
    }
    render(<App data={data} />)
    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }))
    expect(screen.getByText(/job-9/)).toBeInTheDocument()
    expect(screen.getByText(/settled/)).toBeInTheDocument()
    expect(screen.getByText(/request request-1/)).toBeInTheDocument()
    expect(screen.getByText(/call call-3/)).toBeInTheDocument()
    expect(screen.getByText(/tool kind function/)).toBeInTheDocument()
    expect(screen.getByText(/slow_tool/)).toBeInTheDocument()
    expect(screen.getByText(/delivered false/)).toBeInTheDocument()
    expect(screen.getByText(/result pending delivery/)).toBeInTheDocument()
  })

  it('presents raw custom job kind and retained terminal output', () => {
    const data: HarnessViewModel = {
      nodes: [{ id: 'root', name: '/root', state: 'cancelled' }],
      timeline: [{
        id: 'custom-job-4', nodeId: 'root', label: 'custom run', kind: 'job', state: 'cancelled',
        requestId: 'request-4', callId: 'call-custom-4', toolKind: 'custom',
        toolName: 'run', delivered: true, output: 'cancelled; retained result',
      }],
      inbox: [],
    }
    render(<App data={data} />)
    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }))
    expect(screen.getByText(/custom-job-4/)).toBeInTheDocument()
    expect(screen.getByText(/tool kind custom/)).toBeInTheDocument()
    expect(screen.getByText(/call-custom-4/)).toBeInTheDocument()
    expect(screen.getByRole('cell', { name: /^cancelled$/ })).toBeInTheDocument()
    expect(screen.getByText(/delivered true/)).toBeInTheDocument()
    expect(screen.getByText(/cancelled; retained result/)).toBeInTheDocument()
  })

  it('uses explicit hostRun to show embedded controls even when the actor list is empty', () => {
    render(<App data={{ hostRun: 'run-7', actors: [], nodes: [], timeline: [], inbox: [] }} />)
    expect(screen.getByRole('button', { name: 'Host' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Command' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Host' }))
    expect(screen.getByRole('heading', { name: 'Embedded host controls' })).toBeInTheDocument()
    expect(screen.getByText('No actors are currently projected for this host run.')).toBeInTheDocument()
  })

  it('sends input, interrupt, and retire to the explicitly selected exact actor', () => {
    const onCommand = vi.fn()
    const selected = {
      id: '["run-7","/root/reviewer","inc-2"]', name: '/root/reviewer', run: 'run-7', incarnation: 'inc-2',
      kind: 'workflow' as const, lifecycle: 'waiting', activeRound: 'round-8',
    }
    const data: HarnessViewModel = {
      hostRun: 'run-7',
      actors: [
        { ...selected, id: '["run-7","/root/other","inc-1"]', name: '/root/other', incarnation: 'inc-1', kind: 'model', lifecycle: 'running' },
        selected,
        { ...selected, id: '["different-run","/root/foreign","inc-1"]', name: '/root/foreign', run: 'different-run', incarnation: 'inc-1', kind: 'model', lifecycle: 'running' },
      ],
      nodes: [], timeline: [], inbox: [],
    }
    render(<App data={data} onCommand={onCommand} />)
    fireEvent.click(screen.getByRole('button', { name: 'Host' }))
    const selector = screen.getByLabelText('Target actor')
    expect(selector.querySelectorAll('option')).toHaveLength(3)
    fireEvent.change(selector, { target: { value: selected.id } })
    fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: 'continue with care' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send input' }))
    fireEvent.click(screen.getByRole('button', { name: 'Interrupt' }))
    fireEvent.click(screen.getByRole('button', { name: 'Retire' }))
    expect(onCommand.mock.calls.map(([command]) => command)).toEqual([
      { action: 'input', target: { run: 'run-7', actor: '/root/reviewer', incarnation: 'inc-2' }, text: 'continue with care' },
      { action: 'interrupt', target: { run: 'run-7', actor: '/root/reviewer', incarnation: 'inc-2' }, expected_round: 'round-8' },
      { action: 'retire', target: { run: 'run-7', actor: '/root/reviewer', incarnation: 'inc-2' } },
    ])
  })

  it('keeps a selected actor unavailable after an incarnation replacement until reselected', () => {
    const onCommand = vi.fn()
    const first = {
      id: '["run-7","/root/reviewer","inc-1"]', name: '/root/reviewer', run: 'run-7', incarnation: 'inc-1',
      kind: 'workflow' as const, lifecycle: 'waiting',
    }
    const replacement = { ...first, id: '["run-7","/root/reviewer","inc-2"]', incarnation: 'inc-2' }
    const view = (actor: typeof first): HarnessViewModel => ({ hostRun: 'run-7', actors: [actor], nodes: [], timeline: [], inbox: [] })
    const { rerender } = render(<App data={view(first)} onCommand={onCommand} />)
    fireEvent.click(screen.getByRole('button', { name: 'Host' }))
    const selector = screen.getByLabelText('Target actor') as HTMLSelectElement
    fireEvent.change(selector, { target: { value: first.id } })
    rerender(<App data={view(replacement)} onCommand={onCommand} />)
    expect(selector.value).toBe(first.id)
    expect(screen.getByText(/selected actor disappeared or was replaced/i)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Send input' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Interrupt' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Retire' })).toBeDisabled()
    expect(onCommand).not.toHaveBeenCalled()

    fireEvent.change(selector, { target: { value: replacement.id } })
    fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: 'address replacement' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send input' }))
    expect(onCommand).toHaveBeenCalledWith({
      action: 'input', target: { run: 'run-7', actor: '/root/reviewer', incarnation: 'inc-2' }, text: 'address replacement',
    })
  })

  it('retains the standalone command view when hostRun is absent', () => {
    const onCommand = vi.fn()
    render(<App data={{ actors: [], nodes: [], timeline: [], inbox: [] }} onCommand={onCommand} />)
    expect(screen.getByRole('button', { name: 'Command' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Host' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Command' }))
    expect(screen.getByText(/Deterministic mode/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Run test' }))
    expect(onCommand).toHaveBeenCalledWith('test')
  })

  it('shows handoff outcomes without treating control requests as completed work', () => {
    const data: HarnessViewModel = {
      hostRun: 'run-7', actors: [], nodes: [], timeline: [], inbox: [],
      commandReceipts: [
        { commandId: 'cmd-input', outcome: 'admitted', envelopeId: 'env-8' },
        {
          commandId: 'cmd-interrupt', target: { run: 'run-7', actor: '/root/worker', incarnation: 'inc-2' },
          outcome: 'control_requested', control: 'interrupt',
        },
        { commandId: 'cmd-refused', outcome: 'refused', reason: 'target actor is no longer live' },
      ],
    }
    render(<App data={data} />)
    fireEvent.click(screen.getByRole('button', { name: 'Host' }))
    expect(screen.getByText('Admitted for processing')).toBeInTheDocument()
    expect(screen.getByText('Interrupt requested')).toBeInTheDocument()
    expect(screen.getByText('Refused')).toBeInTheDocument()
    expect(screen.getByText(/request sent to the host; this does not report actor completion/i)).toBeInTheDocument()
    expect(screen.queryByText('Completed')).not.toBeInTheDocument()
  })
})
