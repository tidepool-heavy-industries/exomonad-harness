import { describe, expect, it } from 'vitest'
import { render, screen } from '@testing-library/react'
import { isRichView, RichViewRenderer } from './rich-view'

describe('rich actor views', () => {
  it('renders text as text and keeps SVG out of the DOM parser', () => {
    const hostile = '<img src=x onerror=alert(1)>'
    const view = { kind: 'column', children: [{ kind: 'text', text: hostile }, { kind: 'svg', source: '<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>' }] } as const
    expect(isRichView(view)).toBe(true)
    const { container } = render(<RichViewRenderer view={view} />)
    expect(screen.getByText(hostile)).toBeTruthy()
    expect(container.querySelector('img[onerror]')).toBeNull()
    expect(container.querySelector('script')).toBeNull()
    expect(container.querySelector('img')?.getAttribute('src')).toContain('data:image/svg+xml,')
  })
  it('rejects unknown kinds and unsafe image media types', () => {
    expect(isRichView({ kind: 'html', text: '<b>x</b>' })).toBe(false)
    expect(isRichView({ kind: 'image', alt: 'x', source: { kind: 'data', mime: 'image/svg+xml', base64: 'PHN2Zz4=' } })).toBe(false)
  })
  it('marks bounded previews and unavailable inspection details', () => {
    const view = { kind: 'column' as const, children: [
      { kind: 'text' as const, text: 'preview', truncated: true },
      { kind: 'inspection' as const, text: 'partial details', has_more: true, unavailable: true },
    ] }
    render(<RichViewRenderer view={view} />)
    expect(screen.getByText('Value truncated.')).toBeTruthy()
    expect(screen.getByText('More detail is available.')).toBeTruthy()
    expect(screen.getByText('Some detail is unavailable.')).toBeTruthy()
  })
})
