import { useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import Markdown, { type Components } from 'react-markdown'
import './MessagePresentation.css'

export type MessageKind = 'assistant' | 'human' | 'reasoning' | 'activity'

export interface MessagePresentationProps {
  readonly kind: MessageKind
  readonly title: string
  readonly children: ReactNode
  readonly status?: ReactNode
  readonly details?: ReactNode
  readonly listItem?: boolean
  readonly collapsed?: boolean
}

/** Content identity and lifecycle remain the caller's responsibility. */
export function MessagePresentation({ kind, title, children, status, details, listItem = true, collapsed = false }: MessagePresentationProps) {
  const heading = <h3>{title}</h3>
  return <article role={listItem ? 'listitem' : undefined} className={`chat-message chat-message-${kind}`}>
    {collapsed ? <details className="chat-message-fold">
      <summary className="chat-message-header">{heading}{status && <span className="chat-message-status">{status}</span>}</summary>
      <div className="chat-message-body">{children}</div>
    </details> : <>
      <header className="chat-message-header">{heading}{status && <span className="chat-message-status">{status}</span>}</header>
      <div className="chat-message-body">{children}</div>
    </>}
    {details && <details className="chat-message-details"><summary>Message details</summary>{details}</details>}
  </article>
}

function CopyButton({ text, label, children }: { readonly text: string; readonly label: string; readonly children: ReactNode }) {
  const [result, setResult] = useState<{ text: string; state: 'copied' | 'failed' }>()
  async function copy() {
    try {
      if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable')
      await navigator.clipboard.writeText(text)
      setResult({ text, state: 'copied' })
    } catch {
      setResult({ text, state: 'failed' })
    }
  }
  const state = result?.text === text ? result.state : undefined
  return <>
    <button type="button" aria-label={label} onClick={() => void copy()}>{state === 'copied' ? 'Copied' : children}</button>
    {state === 'copied' && <span className="message-copy-status" role="status">Copied to clipboard.</span>}
    {state === 'failed' && <span className="message-copy-status" role="status">Copy unavailable. Select the text to copy.</span>}
  </>
}

export interface MessageContentProps {
  readonly text: string
  /** Original retained string when text is a formatted presentation. */
  readonly copyText?: string
  readonly label: string
  readonly format?: 'markdown' | 'plain'
  readonly copy?: boolean
}

/** Collapse the reading viewport, never the retained source or Markdown input. */
export function MessageContent({ text, copyText = text, label, format = 'plain', copy = true }: MessageContentProps) {
  const [expanded, setExpanded] = useState(false)
  const [overflowing, setOverflowing] = useState(false)
  const viewport = useRef<HTMLDivElement>(null)
  const body = useRef<HTMLDivElement>(null)
  const id = useId()
  useLayoutEffect(() => {
    if (expanded) return
    const measure = () => {
      const element = viewport.current
      if (element) setOverflowing(element.scrollHeight > element.clientHeight + 1)
    }
    measure()
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measure)
    if (body.current) observer?.observe(body.current)
    window.addEventListener('resize', measure)
    return () => { observer?.disconnect(); window.removeEventListener('resize', measure) }
  }, [text, format, expanded])
  const components: Components = {
    pre({ children, node }) {
      // Markdown's code text is the rendered block, including its final newline.
      const code = node?.children.find(child => child.type === 'element' && child.tagName === 'code')
      const codeText = code?.type === 'element' ? code.children.map(child => child.type === 'text' ? child.value : '').join('') : ''
      return <div className="message-code-block">
        <div className="message-code-actions"><CopyButton text={codeText} label={`Copy code from ${label}`}>Copy code</CopyButton></div>
        <pre>{children}</pre>
      </div>
    },
  }
  return <div className={`message-content message-content-${format}`}>
    <div className={`message-content-viewport${expanded ? ' message-content-expanded' : ''}`} ref={viewport}
      onFocusCapture={() => { if (overflowing) setExpanded(true) }}>
      <div id={id} ref={body} aria-label={label} className="message-content-body">
        {format === 'markdown' ? <Markdown skipHtml components={components}>{text}</Markdown> : <pre className="history-content">{text}</pre>}
      </div>
    </div>
    <div className="message-content-actions">
      {overflowing && <button type="button" aria-expanded={expanded} aria-controls={id} onClick={() => setExpanded(value => !value)}>
        {expanded ? 'Collapse' : 'Expand'} {label}
      </button>}
      {copy && <CopyButton text={copyText} label={`Copy ${label}`}>Copy text</CopyButton>}
    </div>
    {overflowing && <p className="message-content-note">{expanded ? 'Showing all fetched content.' : 'Reading view is collapsed. Expand to see all fetched content.'} Copy retains the complete text.</p>}
  </div>
}
