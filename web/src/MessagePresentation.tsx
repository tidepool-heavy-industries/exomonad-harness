import { useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import Markdown, { type Components } from 'react-markdown'
import './MessagePresentation.css'
import { PresentationDetails, usePresentationChoice } from './presentation-state'

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
  const Element = listItem ? 'div' : 'article'
  return <Element role={listItem ? 'listitem' : undefined} className={`chat-message chat-message-${kind}`}>
    {collapsed ? <PresentationDetails memoryKey={`fold:${kind}`} className="chat-message-fold"
      summary={<span className="chat-message-header">{heading}{status && <span className="chat-message-status">{status}</span>}</span>}>
      <div className="chat-message-body">{children}</div>
    </PresentationDetails> : <>
      <header className="chat-message-header">{heading}{status && <span className="chat-message-status">{status}</span>}</header>
      <div className="chat-message-body">{children}</div>
    </>}
    {details && <PresentationDetails memoryKey="evidence" className="chat-message-details" summary="Message details">{details}</PresentationDetails>}
  </Element>
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
  readonly bounded?: boolean
  readonly memoryKey?: string
}

function MarkdownCode({ children, text, label, offset }: { children: ReactNode; text: string; label: string; offset: number }) {
  const [expanded, setExpanded] = usePresentationChoice(`code:${offset}`)
  const [overflowing, setOverflowing] = useState(false)
  const pre = useRef<HTMLPreElement>(null)
  const id = useId()
  useLayoutEffect(() => {
    if (expanded) return
    const measure = () => { if (pre.current) setOverflowing(pre.current.scrollHeight > pre.current.clientHeight + 1) }
    measure()
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measure)
    if (pre.current) observer?.observe(pre.current)
    return () => observer?.disconnect()
  }, [text, expanded])
  return <div className="message-code-block">
    <div className="message-code-actions">
      {overflowing && <button type="button" aria-expanded={expanded} aria-controls={id} onClick={() => setExpanded(!expanded)}>{expanded ? 'Collapse code' : 'Expand code'}</button>}
      <CopyButton text={text} label={`Copy code from ${label}`}>Copy code</CopyButton>
    </div>
    <pre ref={pre} id={id} tabIndex={0} className={expanded ? 'code-expanded' : 'code-preview'} onFocus={() => { if (overflowing) setExpanded(true) }}>{children}</pre>
  </div>
}

/** Collapse the reading viewport, never the retained source or Markdown input. */
export function MessageContent({ text, copyText = text, label, format = 'plain', copy = true, bounded = format === 'plain', memoryKey = label }: MessageContentProps) {
  const [expanded, setExpanded] = usePresentationChoice(`content:${memoryKey}`)
  const [overflowing, setOverflowing] = useState(false)
  const viewport = useRef<HTMLDivElement>(null)
  const body = useRef<HTMLDivElement>(null)
  const id = useId()
  useLayoutEffect(() => {
    if (expanded || !bounded) return
    const measure = () => {
      const element = viewport.current
      if (element) setOverflowing(element.scrollHeight > element.clientHeight + 1)
    }
    measure()
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measure)
    if (body.current) observer?.observe(body.current)
    window.addEventListener('resize', measure)
    return () => { observer?.disconnect(); window.removeEventListener('resize', measure) }
  }, [text, format, expanded, bounded])
  const components = useMemo<Components>(() => ({
    h1: ({ children }) => <h4>{children}</h4>,
    h2: ({ children }) => <h5>{children}</h5>,
    h3: ({ children }) => <h6>{children}</h6>,
    h4: ({ children }) => <h6>{children}</h6>,
    h5: ({ children }) => <h6>{children}</h6>,
    h6: ({ children }) => <h6>{children}</h6>,
    pre({ children, node }) {
      // Markdown's code text is the rendered block, including its final newline.
      const code = node?.children.find(child => child.type === 'element' && child.tagName === 'code')
      const codeText = code?.type === 'element' ? code.children.map(child => child.type === 'text' ? child.value : '').join('') : ''
      return <MarkdownCode text={codeText} label={label} offset={node?.position?.start.offset ?? 0}>{children}</MarkdownCode>
    },
  }), [label])
  return <div className={`message-content message-content-${format}`}>
    <div className={`message-content-viewport${expanded || !bounded ? ' message-content-expanded' : ''}`} ref={viewport}
      onFocusCapture={() => { if (overflowing) setExpanded(true) }}>
      <div id={id} ref={body} aria-label={label} className="message-content-body">
        {format === 'markdown' ? <Markdown skipHtml components={components}>{text}</Markdown> : <pre className="history-content" tabIndex={bounded ? 0 : undefined}>{text}</pre>}
      </div>
    </div>
    <div className="message-content-actions">
      {bounded && overflowing && <button type="button" aria-expanded={expanded} aria-controls={id} onClick={() => setExpanded(!expanded)}>
        {expanded ? 'Collapse' : 'Expand'} {label}
      </button>}
      {copy && <CopyButton text={copyText} label={`Copy ${label}`}>Copy text</CopyButton>}
    </div>
    {bounded && overflowing && <p className="message-content-note">{expanded ? 'Full content shown.' : 'Preview shown.'} Copy includes the full text.</p>}
  </div>
}
