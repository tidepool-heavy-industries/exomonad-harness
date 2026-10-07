import type { ReactNode } from 'react'
import Markdown from 'react-markdown'

export type RichView =
  | { readonly kind: 'text' | 'markdown'; readonly text: string; readonly truncated?: boolean }
  | { readonly kind: 'row' | 'column'; readonly children: readonly RichView[] }
  | { readonly kind: 'caption'; readonly body: RichView; readonly text: string; readonly truncated?: boolean }
  | { readonly kind: 'svg'; readonly source: string }
  | { readonly kind: 'image'; readonly source: ImageSource; readonly alt: string; readonly truncated?: boolean }
  | { readonly kind: 'inspection'; readonly text: string; readonly has_more: boolean; readonly unavailable: boolean }

export type ImageSource =
  | { readonly kind: 'data'; readonly mime: 'image/png' | 'image/jpeg' | 'image/webp'; readonly base64: string }
  | { readonly kind: 'retained'; readonly hash: string }

type Obj = Record<string, unknown>
const obj = (value: unknown): value is Obj => typeof value === 'object' && value !== null && !Array.isArray(value)
const bounded = (value: unknown, max: number) => typeof value === 'string' && new TextEncoder().encode(value).length <= max

export function isRichView(value: unknown, depth = 0): value is RichView {
  if (!obj(value) || depth > 32 || typeof value.kind !== 'string') return false
  if (Object.hasOwn(value, 'truncated') && typeof value.truncated !== 'boolean') return false
  switch (value.kind) {
    case 'text': case 'markdown': return bounded(value.text, 256 * 1024)
    case 'row': case 'column': return Array.isArray(value.children) && value.children.length <= 1024
      && value.children.every(child => isRichView(child, depth + 1))
    case 'caption': return bounded(value.text, 8192) && isRichView(value.body, depth + 1)
    case 'svg': return bounded(value.source, 128 * 1024)
    case 'image': return bounded(value.alt, 8192) && isImageSource(value.source)
    case 'inspection': return bounded(value.text, 256 * 1024) && typeof value.has_more === 'boolean' && typeof value.unavailable === 'boolean'
    default: return false
  }
}

function isImageSource(value: unknown): value is ImageSource {
  if (!obj(value)) return false
  if (value.kind === 'retained') return typeof value.hash === 'string' && /^[0-9a-f]{64}$/.test(value.hash)
  return value.kind === 'data' && (value.mime === 'image/png' || value.mime === 'image/jpeg' || value.mime === 'image/webp')
    && typeof value.base64 === 'string' && value.base64.length <= 4 * 1024 * 1024 && /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value.base64)
}

function safeSvgData(source: string) {
  return `data:image/svg+xml,${encodeURIComponent(source)}`
}

export function RichViewRenderer({ view, className }: { view: RichView; className?: string }) {
  const node = render(view)
  return <div className={className ? `rich-view ${className}` : 'rich-view'}>{node}</div>
}

function render(view: RichView): ReactNode {
  switch (view.kind) {
    case 'text': return <><pre className="rich-view-text">{view.text}</pre>{view.truncated && <p className="rich-view-truncated" role="status">Value truncated.</p>}</>
    case 'markdown': return <><div className="rich-view-markdown"><Markdown skipHtml>{view.text}</Markdown></div>{view.truncated && <p className="rich-view-truncated" role="status">Value truncated.</p>}</>
    case 'row': return <div className="rich-view-row">{view.children.map((child, i) => <RichViewRenderer key={i} view={child} />)}</div>
    case 'column': return <div className="rich-view-column">{view.children.map((child, i) => <RichViewRenderer key={i} view={child} />)}</div>
    case 'caption': return <figure className="rich-view-caption"><RichViewRenderer view={view.body} /><figcaption>{view.text}</figcaption>{view.truncated && <p className="rich-view-truncated" role="status">Value truncated.</p>}</figure>
    case 'svg': return <img className="rich-view-image" src={safeSvgData(view.source)} alt="SVG output" />
    case 'image': return <><img className="rich-view-image" src={view.source.kind === 'retained'
      ? `/api/actor-media/${encodeURIComponent(view.source.hash)}`
      : `data:${view.source.mime};base64,${view.source.base64}`} alt={view.alt} />{view.truncated && <p className="rich-view-truncated" role="status">Value truncated.</p>}</>
    case 'inspection': return <section className="rich-view-inspection" aria-label="Inspection result"><pre>{view.text}</pre>
      {view.has_more && <p>More detail is available.</p>}{view.unavailable && <p>Some detail is unavailable.</p>}</section>
  }
}
