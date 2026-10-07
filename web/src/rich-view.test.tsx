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
})
