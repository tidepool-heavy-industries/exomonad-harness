import './rich-view.css'
import type { ReactNode } from 'react'
import Markdown from 'react-markdown'

import type { RichView } from './rich-view-model'
export type { RichView, ImageSource } from './rich-view-model'
export { isRichView } from './rich-view-model'

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
