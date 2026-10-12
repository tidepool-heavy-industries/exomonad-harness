import { useState } from 'react'
import HistoryItem from './HistoryItem'
import { MessageContent, MessagePresentation } from './MessagePresentation'
import type { HistoryEntry } from './history-client'
import type { ActorOutputOrigin } from './actor-output'
import type { UnifiedEntry } from './conversation-client'
import { outputKey, outputLabels, originKey, type OutputOrigin } from './live-output'
import type { TimelineRow } from './chat-timeline'
import { RichViewRenderer } from './rich-view'
import ToolActivity, { type ExchangeObservation, type ToolJobObservation } from './ToolActivity'

export function TimelineContent({ row, streamingActive, requests, jobs, modelOrigin, onRevealActivity }: { row: TimelineRow; streamingActive: boolean; requests?: ReadonlyMap<string, ExchangeObservation>; jobs: readonly ToolJobObservation[]; modelOrigin?: OutputOrigin; onRevealActivity: (key: string) => void }) {
  if (row.kind === 'activity' || row.kind === 'live' && row.live.every(item => item.channel === 'tool_input' || item.channel === 'tool_arguments')) {
    const operation = row.kind === 'activity' ? row.entry.tool?.operation : undefined
    const matchedJobs = operation && modelOrigin && originKey(operation.origin) === originKey(modelOrigin)
      ? jobs.filter(job => job.kind === 'job' && job.requestId === operation.request && job.callId === operation.call) : []
    return <ToolActivity row={row} streamingActive={streamingActive} exchange={requests?.get(row.kind === 'activity' ? row.entry.requestId : row.live[0]!.requestId)} jobs={matchedJobs} />
  }
  if (row.kind === 'live') return <div className="live-message">{row.live.map(item => <MessagePresentation key={outputKey(item)} listItem={false}
    kind={item.channel === 'reasoning' || item.channel === 'reasoning_summary' ? 'reasoning' : item.channel === 'tool_input' || item.channel === 'tool_arguments' ? 'activity' : 'assistant'}
    title={outputLabels[item.channel]} collapsed={item.channel === 'reasoning' || item.channel === 'reasoning_summary'}
    status={item.committedHash ? 'Retained preview' : item.streaming && streamingActive ? 'Streaming' : 'Incomplete'}>
    <MessageContent text={item.text} label={`${outputLabels[item.channel]} output`} memoryKey={`part:${row.live.indexOf(item)}`} bounded={item.channel === 'tool_input' || item.channel === 'tool_arguments'} format={item.channel === 'assistant' || item.channel === 'refusal' ? 'markdown' : 'plain'} />
    {item.overflow && <p role="status">Live preview is partial. Completed content remains available in retained history.</p>}
  </MessagePresentation>)}</div>
  if (row.kind === 'result') return row.invocationKey ? <div className="tool-result-marker"><span>Tool result retained</span><button type="button" onClick={() => onRevealActivity(row.invocationKey!)}>Show tool activity</button></div> : <div className="message tool-activity"><h3>Tool result</h3><p className="meta">{row.entry.tool ? 'The exact invocation is outside the loaded window.' : 'Operation ownership is unavailable for this historical result.'}</p><HistoryItem listItem={false} publication={row.entry} entry={historyEntry(row.entry)} /></div>
  if (row.entry.kind === 'message') return <HistoryItem listItem={false} publication={row.entry} entry={historyEntry(row.entry)} />
  if (row.entry.kind === 'oversized') return <p role="status">Message {row.entry.position} is too large to display ({row.entry.byteLen} bytes).</p>
  return null
}
function historyEntry(entry: Extract<UnifiedEntry, { kind: 'message' }>): HistoryEntry { return { position: entry.position, hash: entry.hash, byteLen: String(new TextEncoder().encode(JSON.stringify(entry.item)).length), item: entry.item } }

export function UnifiedOutput({ entry, origin, ready, active, onAuthExpired, onExpanded }: {
  entry: Extract<UnifiedEntry, { kind: 'output' }>; origin: ActorOutputOrigin; ready: boolean; active: boolean
  onAuthExpired?: () => void; onExpanded: () => void
}) {
  const [busyKey, setBusyKey] = useState<string>()
  const [issue, setIssue] = useState('')
  const output = entry.output.emission
  async function expand(key: string) {
    setBusyKey(key)
    try {
      const response = await fetch('/api/actor-output/expand', { method: 'POST', credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ origin, displaySlot: output.id.displaySlot, key }) })
      if (response.status === 401 || response.status === 403) onAuthExpired?.()
      if (!response.ok) throw new Error(response.status === 409 ? 'This detail is no longer available. Refresh the conversation.' : 'This detail could not be expanded.')
      setIssue('')
      onExpanded()
    } catch (error) { setIssue(error instanceof Error ? error.message : String(error)) }
    finally { setBusyKey(undefined) }
  }
  return <><h3>Displayed values</h3>{output.page.view ? <RichViewRenderer view={output.page.view} /> : <pre>{output.page.text}</pre>}
    {issue && <p role="alert">{issue}</p>}
    {output.page.unavailable && <p>Some detail is unavailable from this renderer.</p>}
    {output.page.expansions.map(([key, label]) => <button key={key} disabled={!active || !ready || busyKey !== undefined}
      onClick={() => void expand(key)}>{busyKey === key ? 'Loading…' : `Show ${label}`}</button>)}
  </>
}
