import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import Markdown, { type Components } from 'react-markdown'
import './MessagePresentation.css'
import { PresentationDetails, usePresentationChoice } from './presentation-state'
import { copyText } from './clipboard'
import Code, { type CodeLanguage } from './Code'

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
      await copyText(text)
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
  readonly streaming?: boolean
  readonly language?: CodeLanguage
  readonly copy?: boolean
  readonly bounded?: boolean
  readonly memoryKey?: string
}

function CodeBlock({ children, text, copyText = text, label, memoryKey, copy = true, bounded = true, sourceLabel }: {
  children: ReactNode; text: string; copyText?: string; label: string; memoryKey: string; copy?: boolean; bounded?: boolean; sourceLabel?: string
}) {
  const [expanded, setExpanded] = usePresentationChoice(memoryKey)
  const [overflowing, setOverflowing] = useState(expanded)
  const pre = useRef<HTMLPreElement>(null)
  const id = useId()
  useLayoutEffect(() => {
    if (expanded || !bounded) return
    const measure = () => { if (pre.current) setOverflowing(pre.current.scrollHeight > pre.current.clientHeight + 1) }
    measure()
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measure)
    if (pre.current) observer?.observe(pre.current)
    return () => observer?.disconnect()
  }, [text, expanded, bounded])
  return <div className="message-code-block">
    <div className="message-code-actions">
      {bounded && overflowing && <button type="button" aria-expanded={expanded} aria-controls={id} onClick={() => setExpanded(!expanded)}>{expanded ? 'Collapse' : 'Expand'} {sourceLabel ?? 'code'}</button>}
      {copy && <CopyButton text={copyText} label={sourceLabel ? `Copy ${sourceLabel}` : `Copy code from ${label}`}>Copy code</CopyButton>}
    </div>
    <pre ref={pre} id={id} tabIndex={0} aria-label={sourceLabel} className={expanded || !bounded ? 'code-expanded' : 'code-preview'} onFocus={() => { if (overflowing) setExpanded(true) }}>{children}</pre>
  </div>
}

/** Collapse the reading viewport, never the retained source or Markdown input. */
export function MessageContent({ text, copyText = text, label, format = 'plain', language, streaming = false, copy = true, bounded = format === 'plain', memoryKey = label }: MessageContentProps) {
  const preview = useStreamingText(text, streaming)
  const contentBounded = bounded && !language
  const [expanded, setExpanded] = usePresentationChoice(`content:${memoryKey}`)
  const [overflowing, setOverflowing] = useState(expanded)
  const viewport = useRef<HTMLDivElement>(null)
  const body = useRef<HTMLDivElement>(null)
  const id = useId()
  useLayoutEffect(() => {
    if (expanded || !contentBounded) return
    const measure = () => {
      const element = viewport.current
      if (element) setOverflowing(element.scrollHeight > element.clientHeight + 1)
    }
    measure()
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measure)
    if (body.current) observer?.observe(body.current)
    window.addEventListener('resize', measure)
    return () => { observer?.disconnect(); window.removeEventListener('resize', measure) }
  }, [text, format, expanded, contentBounded])
  const components = useMemo<Components>(() => ({
    h1: ({ children }) => <h4>{children}</h4>,
    h2: ({ children }) => <h5>{children}</h5>,
    h3: ({ children }) => <h6>{children}</h6>,
    h4: ({ children }) => <h6>{children}</h6>,
    h5: ({ children }) => <h6>{children}</h6>,
    h6: ({ children }) => <h6>{children}</h6>,
    code({ children, className }) {
      return typeof children === 'string' && /(?:^|\s)language-(?:haskell|hs)(?:\s|$)/.test(className ?? '')
        ? <Code text={children} language="haskell" /> : <code className={className}>{children}</code>
    },
    pre({ children, node }) {
      // Markdown's code text is the rendered block, including its final newline.
      const code = node?.children.find(child => child.type === 'element' && child.tagName === 'code')
      const codeText = code?.type === 'element' ? code.children.map(child => child.type === 'text' ? child.value : '').join('') : ''
      return <CodeBlock text={codeText} label={label} memoryKey={`code:${node?.position?.start.offset ?? 0}`}>{children}</CodeBlock>
    },
  }), [label])
  const markdown = useMemo(() => format === 'markdown' ? <Markdown skipHtml components={components}>{preview}</Markdown> : null, [format, preview, components])
  const content = useMemo(() => format === 'markdown'
    ? markdown
    : language ? <CodeBlock text={preview} copyText={copyText} label={label} sourceLabel={label} memoryKey={`content:${memoryKey}`} bounded={bounded} copy={copy}>
      <Code text={preview} language={language} />
    </CodeBlock> : <pre className="history-content" tabIndex={bounded ? 0 : undefined}>{preview}</pre>, [preview, copyText, label, memoryKey, copy, format, markdown, bounded, language])
  return <div className={`message-content message-content-${format}`}>
    <div className={`message-content-viewport${expanded || !contentBounded ? ' message-content-expanded' : ''}`} ref={viewport}
      onFocusCapture={() => { if (contentBounded && overflowing) setExpanded(true) }}>
      <div id={id} ref={body} aria-label={language ? undefined : label} className="message-content-body">
        {content}
      </div>
    </div>
    {!language && <div className="message-content-actions">
      {contentBounded && overflowing && <button type="button" aria-expanded={expanded} aria-controls={id} onClick={() => setExpanded(!expanded)}>
        {expanded ? 'Collapse' : 'Expand'} {label}
      </button>}
      {copy && <CopyButton text={copyText} label={`Copy ${label}`}>Copy text</CopyButton>}
    </div>}
    {contentBounded && overflowing && <p className="message-content-note">{expanded ? 'Full content shown.' : 'Preview shown.'} Copy includes the full text.</p>}
  </div>
}

/** Coalesce live parsing/highlighting, whose inputs are bounded by the output owner.
 * Copy always uses the latest source; completion renders immediately. A fixed
 * cadence avoids re-tokenizing the accumulated prefix for every provider token.
 */
export function useStreamingText(text: string, streaming: boolean): string {
  const [preview, setPreview] = useState(text)
  const latest = useRef(text)
  latest.current = text
  useEffect(() => {
    if (!streaming) return
    const timer = window.setInterval(() => setPreview(latest.current), 80)
    return () => window.clearInterval(timer)
  }, [streaming])
  return streaming ? preview : text
}
