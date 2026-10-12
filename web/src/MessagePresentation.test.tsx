import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MessageContent } from './MessagePresentation'

const clipboardDescriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
afterEach(() => {
  vi.restoreAllMocks()
  if (clipboardDescriptor) Object.defineProperty(navigator, 'clipboard', clipboardDescriptor)
  else Reflect.deleteProperty(navigator, 'clipboard')
})

function clipboard(writeText = vi.fn().mockResolvedValue(undefined)) {
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
  return writeText
}

function overflowingViewport() {
  // JSDOM has no layout. Model a content body taller than the 24rem viewport.
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(312)
  vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockReturnValue(600)
}

describe('message presentation', () => {
  it('renders Markdown paragraphs, lists, emphasis and code with safe links and no raw HTML', () => {
    const text = '# Result\n\nFirst **strong** paragraph with `inline`.\n\nSecond paragraph.\n\n- one\n- two\n\n[safe](https://example.com) [unsafe](javascript:alert%281%29)\n\n<script>alert(1)</script>\n\n<iframe src="https://example.com"></iframe>'
    const { container } = render(<MessageContent text={text} label="Answer" format="markdown" />)
    expect(screen.getByRole('heading', { name: 'Result' })).toBeInTheDocument()
    expect(container.querySelector('strong')?.textContent).toBe('strong')
    expect(container.querySelector('code')?.textContent).toBe('inline')
    expect(container.querySelectorAll('li')).toHaveLength(2)
    expect(screen.getByRole('link', { name: 'safe' })).toHaveAttribute('href', 'https://example.com')
    expect(screen.getByText('unsafe').getAttribute('href')).toBe('')
    expect(container.querySelector('script, iframe')).toBeNull()
  })

  it('copies exact code and source, preserving code feedback while streamed source changes', async () => {
    const writeText = clipboard()
    const code = 'const cat = "🐈"\n  next(λ)\n'
    const text = `Paragraph **one**.\n\n\`\`\`typescript\n${code}\`\`\`\n\nLast paragraph.`
    const { container, rerender } = render(<MessageContent text={text} label="Answer" format="markdown" />)
    expect(container.querySelector('pre code')?.textContent).toBe(code)
    fireEvent.click(screen.getByRole('button', { name: 'Copy code from Answer' }))
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(code))
    const codeStatus = screen.getByRole('status')
    fireEvent.click(screen.getByRole('button', { name: 'Copy Answer' }))
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(text))
    const copySource = screen.getByRole('button', { name: 'Copy Answer' })
    await waitFor(() => expect(within(copySource.parentElement!).getByRole('status')).toBeVisible())
    const updated = `${text}\n\nNext streamed paragraph.`
    rerender(<MessageContent text={updated} label="Answer" format="markdown" />)
    expect(codeStatus).toBeVisible()
    expect(within(copySource.parentElement!).queryByRole('status')).toBeNull()
    fireEvent.click(copySource)
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(updated))
  })

  it('retains whitespace, Unicode and HTML as inert plain source', async () => {
    const writeText = clipboard()
    const text = '  λ 🐈\r\n  <script>x</script>\n**literal**  '
    const { container, rerender } = render(<MessageContent text={text} label="Input" />)
    expect(screen.getByLabelText('Input').textContent).toBe(text)
    expect(container.querySelector('script, strong')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Copy Input' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(text))
    rerender(<MessageContent text={text} label="Input" copy={false} />)
    expect(screen.queryByRole('button')).toBeNull()
    expect(screen.getByLabelText('Input').textContent).toBe(text)
  })

  it.each([false, true])('reports unavailable or refused clipboard writes (refused: %s)', async refused => {
    if (refused) clipboard(vi.fn().mockRejectedValue(new Error('denied')))
    else Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined })
    render(<MessageContent text="Still retained" label="Answer" />)
    fireEvent.click(screen.getByRole('button', { name: 'Copy Answer' }))
    expect(await screen.findByRole('status')).toHaveTextContent('Copy unavailable. Select the text to copy.')
    expect(screen.getByLabelText('Answer')).toHaveTextContent('Still retained')
  })

  it('copies the retained source when display text is formatted and updates its copy state by source identity', async () => {
    const writeText = clipboard()
    const copyText = ' { "cat" : "🐈 λ" }\n'
    const text = '{\n  "cat": "🐈 λ"\n}'
    const { rerender } = render(<MessageContent text={text} copyText={copyText} label="Result" />)
    expect(screen.getByLabelText('Result').textContent).toBe(text)
    fireEvent.click(screen.getByRole('button', { name: 'Copy Result' }))
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(copyText))
    await screen.findByText('Copied to clipboard.')
    rerender(<MessageContent text={text} copyText={`${copyText}\n`} label="Result" />)
    expect(screen.queryByText('Copied to clipboard.')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Copy Result' }))
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(`${copyText}\n`))
  })

  it('collapses by measured display height while preserving complete Unicode source and whole code blocks', async () => {
    overflowingViewport()
    const writeText = clipboard()
    const code = `${'x'.repeat(1999)}🐈\n${'next(λ)\n'.repeat(100)}`
    const text = `\`\`\`text\n${code}\`\`\`\n\n[Retained tail](https://example.com).`
    const { container } = render(<MessageContent text={text} label="Long answer" format="markdown" />)
    expect(screen.getByRole('button', { name: 'Expand Long answer' })).toHaveAttribute('aria-expanded', 'false')
    expect(container.querySelector('pre code')?.textContent).toBe(code)
    fireEvent.click(screen.getByRole('button', { name: 'Expand Long answer' }))
    expect(screen.getByRole('button', { name: 'Collapse Long answer' })).toHaveAttribute('aria-expanded', 'true')
    expect(container.querySelector('pre code')?.textContent).toBe(code)
    fireEvent.click(screen.getByRole('button', { name: 'Copy Long answer' }))
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(text))
    fireEvent.click(screen.getByRole('button', { name: 'Collapse Long answer' }))
    expect(screen.getByRole('button', { name: 'Expand Long answer' })).toHaveAttribute('aria-expanded', 'false')
    fireEvent.focus(screen.getByRole('link', { name: 'Retained tail' }))
    expect(screen.getByRole('button', { name: 'Collapse Long answer' })).toHaveAttribute('aria-expanded', 'true')
    expect(container.querySelector('pre code')?.textContent).toBe(code)
  })

})
