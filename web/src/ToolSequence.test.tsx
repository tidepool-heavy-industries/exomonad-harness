import { fireEvent, render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import ToolSequence from './ToolSequence'
import type { TimelineRow } from './chat-timeline'
import type { ToolJobObservation } from './ToolActivity'
const origin = { kind: 'embedded' as const, run: 'r', actor: '/root', incarnation: '1' }
const entry = { kind: 'message' as const, sequence: '1', requestId: 'req', position: '0', hash: 'a'.repeat(64), inputOperation: null, providerItemId: null,
  tool: { operation: { origin, request: 'req', call: 'call' }, invocation: { request: 'req', position: '0', hash: 'a'.repeat(64) } },
  item: { type: 'custom_tool_call', name: 'haskell', call_id: 'call', input: 'observe value' } }
const result = { ...entry, sequence: '2', item: { type: 'custom_tool_call_output', call_id: 'call', output: 'Compilation failed\nAn exact diagnostic' } }
const rows: TimelineRow[] = [{ kind: 'activity', key: 'call', entry, results: [result] }, { kind: 'result', key: 'result', entry: result, invocationKey: 'call' }]
it('keeps result excerpts and typed failures visible when closed and preserves an expanded group through updates', () => {
  const jobs: ToolJobObservation[] = [{ id: 'job', kind: 'job', nodeId: 'conv', requestId: 'req', callId: 'call', label: 'job', state: 'failed' }]
  const renderRow = (row: TimelineRow) => <div key={row.key}>{row.key} source</div>
  const mounted = render(<ToolSequence rows={rows} renderRow={renderRow} requests={new Map()} jobs={jobs} modelOrigin={origin} streamingActive />)
  expect(screen.getByText(/: Compilation failed$/)).toBeVisible()
  expect(screen.getByText('1 failed job')).toBeVisible()
  expect(screen.getByText('call source')).not.toBeVisible()
  fireEvent.click(screen.getByText('1 tool call · 1 result'))
  const body = screen.getByText('call source')
  expect(body).toBeVisible()
  mounted.rerender(<ToolSequence rows={rows} renderRow={renderRow} requests={new Map([['req', { id: 'req', state: 'failed' }]])} jobs={jobs} modelOrigin={origin} streamingActive />)
  expect(screen.getByText('call source')).toBe(body)
  expect(body).toBeVisible()
  expect(screen.getByText('Response interrupted')).toBeVisible()
})
it('matches failed orphan results only to the exact operation owner', () => {
  const jobs: ToolJobObservation[] = [{ id: 'job', kind: 'job', nodeId: 'conv', requestId: 'req', callId: 'call', label: 'job', state: 'failed' }]
  const orphan = rows.filter(row => row.kind === 'result')
  const mounted = render(<ToolSequence rows={orphan} renderRow={() => null} jobs={jobs} modelOrigin={{ ...origin, incarnation: 'other' }} streamingActive />)
  expect(screen.queryByText('1 failed job')).toBeNull()
  mounted.rerender(<ToolSequence rows={orphan} renderRow={() => null} jobs={jobs.map(job => ({ ...job, callId: 'other' }))} modelOrigin={origin} streamingActive />)
  expect(screen.queryByText('1 failed job')).toBeNull()
  mounted.rerender(<ToolSequence rows={orphan} renderRow={() => null} jobs={jobs} modelOrigin={origin} streamingActive />)
  expect(screen.getByText('1 failed job')).toBeVisible()
})
it('bounds literal result previews while retaining every result in the disclosure', () => {
  const many: TimelineRow[] = Array.from({ length: 50 }, (_, index) => ({ kind: 'result', key: `result-${index}`, entry: { ...result, sequence: String(index + 1), item: { ...result.item, output: `Retained result ${index}` } }, invocationKey: 'call' }))
  render(<ToolSequence rows={many} renderRow={row => <div key={row.key}>{row.kind === 'result' ? `Retained result ${Number(row.entry.sequence) - 1}` : ''}</div>} jobs={[]} streamingActive />)
  expect(screen.getByText('Tool results · 50 results')).toBeVisible()
  expect(screen.getByText('47 earlier result excerpts inside')).toBeVisible()
  expect(screen.getAllByText(/: Retained result/)).toHaveLength(3)
  expect(screen.getByText('Retained result 0')).not.toBeVisible()
})
it('selects latest result publications independently of invocation completion order', () => {
  const sequences = ['9007199254740999', '9007199254740995', '9007199254740996', '9007199254740997']
  const calls: TimelineRow[] = sequences.map((sequence, index) => ({ kind: 'activity', key: `call-${index}`, entry: { ...entry, sequence: String(index + 1) }, results: [{ ...result, sequence, item: { ...result.item, output: `Publication ${sequence}` } }] }))
  const results: TimelineRow[] = [...sequences].sort().map(sequence => ({ kind: 'result', key: sequence, entry: { ...result, sequence, item: { ...result.item, output: `Publication ${sequence}` } }, invocationKey: 'call' }))
  render(<ToolSequence rows={[...calls, ...results]} renderRow={() => null} jobs={[]} streamingActive />)
  expect(screen.queryByText(/: Publication 9007199254740995$/)).toBeNull()
  expect(screen.getAllByText(/: Publication/).map(element => element.textContent)).toEqual(['Result 9007199254740996: Publication 9007199254740996', 'Result 9007199254740997: Publication 9007199254740997', 'Result 9007199254740999: Publication 9007199254740999'])
})

it('separates a future attached result from the group where it is actually published', () => {
  const renderRow = () => null
  const mounted = render(<ToolSequence rows={[rows[0]!]} renderRow={renderRow} jobs={[]} streamingActive={false} />)
  expect(screen.getByText('1 tool call')).toBeVisible()
  expect(screen.getByText('Call 1')).toBeVisible()
  expect(screen.getByText('haskell')).toBeVisible()
  expect(screen.getByText('observe value')).toBeVisible()
  expect(screen.queryByText(/Compilation failed/)).toBeNull()
  mounted.rerender(<ToolSequence rows={[{ ...rows[1]!, invocation: entry } as TimelineRow]} renderRow={renderRow} jobs={[]} streamingActive={false} />)
  expect(screen.getByText('Tool results · 1 result')).toBeVisible()
  expect(screen.getByText('Result for call 1')).toBeVisible()
  expect(screen.getByText(/: Compilation failed$/)).toBeVisible()
})
it('uses certified model output for closed result previews and keeps raw envelopes out of ordinary reading', () => {
  const nativeResult = { ...result, item: { ...result.item, output: 'canonical native envelope' }, modelItem: { ...result.item, output: 'Readable authored result' } }
  render(<ToolSequence rows={[{ kind: 'result', key: 'native', entry: nativeResult }]} renderRow={() => null} jobs={[]} streamingActive={false} />)
  expect(screen.getByText(/: Readable authored result$/)).toBeVisible()
  expect(screen.queryByText(/canonical native envelope/)).toBeNull()
})
it('shows only observed job states for an exact operation and never guesses pending from missing output', () => {
  const callRows = [rows[0]!]
  const jobs: ToolJobObservation[] = [{ id: 'job', kind: 'job', nodeId: 'conv', requestId: 'req', callId: 'call', label: 'job', state: 'running' }]
  const mounted = render(<ToolSequence rows={callRows} renderRow={() => null} jobs={[]} modelOrigin={origin} streamingActive />)
  expect(screen.queryByText(/running job/)).toBeNull()
  mounted.rerender(<ToolSequence rows={callRows} renderRow={() => null} jobs={jobs} modelOrigin={{ ...origin, incarnation: 'other' }} streamingActive />)
  expect(screen.queryByText(/running job/)).toBeNull()
  mounted.rerender(<ToolSequence rows={callRows} renderRow={() => null} jobs={jobs} modelOrigin={origin} streamingActive />)
  expect(screen.getByText('1 running job')).toBeVisible()
})
