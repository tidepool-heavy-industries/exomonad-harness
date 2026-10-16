import type { ChatOperation } from './generated/chat'
import { readable } from './HistoryItem'
import { MessageContent } from './MessagePresentation'

/** The bounded authored view is separate from canonical evidence retained in Store. */
export default function OversizedOutput({ entry }: { entry: { position: string; hash: string; byteLen: string; modelItem?: unknown; outputOperation?: ChatOperation | null } }) {
  const view = readable(entry.modelItem)
  return <div className="message">
    {view?.texts.map((part, index) => <MessageContent key={index} text={part.text} copyText={part.copyText}
      format={part.format} language={part.language} label={`Result item ${entry.position}`} memoryKey={`oversized:${entry.position}:${index}`} bounded />)}
    <p role="status">Canonical item {entry.position} is too large to show ({entry.byteLen} bytes). Full evidence remains in Store.</p>
    {entry.modelItem !== undefined && !view && <p>The authored presentation cannot be displayed by this view.</p>}
    <details className="chat-message-details"><summary>Retained result reference</summary>
      <p>Item hash: <span className="mono">{entry.hash}</span></p>
      {entry.outputOperation && <>
        <p>Use inspectResult with this exact operation reference.</p>
        <p>Request: <span className="mono">{entry.outputOperation.request}</span></p>
        <p>Call: <span className="mono">{entry.outputOperation.call}</span></p>
        <p>Origin: <span className="mono">{JSON.stringify(entry.outputOperation.origin)}</span></p>
      </>}
    </details>
  </div>
}
