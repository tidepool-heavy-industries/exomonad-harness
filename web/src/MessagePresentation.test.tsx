import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MessageContent, MessagePresentation } from './MessagePresentation'

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
    expect(container.querySelectorAll('p').length).toBeGreaterThanOrEqual(3)
    expect(container.querySelector('strong')?.textContent).toBe('strong')
    expect(container.querySelector('code')?.textContent).toBe('inline')
    expect(container.querySelectorAll('li')).toHaveLength(2)
    expect(screen.getByRole('link', { name: 'safe' })).toHaveAttribute('href', 'https://example.com')
    expect(screen.getByText('unsafe').getAttribute('href')).toBe('')
    expect(container.querySelector('script, iframe')).toBeNull()
  })

  it('copies exact source separately from rendered fenced code', async () => {
    const writeText = clipboard()
    const code = 'const cat = "🐈"\n  next(λ)\n'
    const text = `Paragraph **one**.\n\n\`\`\`typescript\n${code}\`\`\`\n\nLast paragraph.`
    const { container } = render(<MessageContent text={text} label="Answer" format="markdown" />)
    expect(container.querySelector('pre code')?.textContent).toBe(code)
    expect(container.querySelector('pre code')).toHaveClass('language-typescript')
    fireEvent.click(screen.getByRole('button', { name: 'Copy code from Answer' }))
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(code))
    fireEvent.click(screen.getByRole('button', { name: 'Copy Answer' }))
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(text))
    expect(await screen.findAllByText('Copied to clipboard.')).toHaveLength(2)
  })

  it('retains whitespace, Unicode and HTML as inert plain source', async () => {
    const writeText = clipboard()
    const text = '  λ 🐈\r\n  <script>x</script>\n**literal**  '
    const { container } = render(<MessageContent text={text} label="Input" />)
    expect(screen.getByLabelText('Input').textContent).toBe(text)
    expect(container.querySelector('script, strong')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Copy Input' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(text))
  })

  it.each([false, true])('reports unavailable or refused clipboard writes (refused: %s)', async refused => {
    if (refused) clipboard(vi.fn().mockRejectedValue(new Error('denied')))
    else Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined })
    render(<MessageContent text="Still retained" label="Answer" />)
    fireEvent.click(screen.getByRole('button', { name: 'Copy Answer' }))
    expect(await screen.findByRole('status')).toHaveTextContent('Copy unavailable. Select the text to copy.')
    expect(screen.getByLabelText('Answer')).toHaveTextContent('Still retained')
  })

  it('copies the current source after live content changes', async () => {
    const writeText = clipboard()
    const { rerender } = render(<MessageContent text="First λ" label="Live answer" />)
    fireEvent.click(screen.getByRole('button', { name: 'Copy Live answer' }))
    await screen.findByText('Copied to clipboard.')
    rerender(<MessageContent text="First λ, then 🐈" label="Live answer" />)
    expect(screen.queryByText('Copied to clipboard.')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Copy Live answer' }))
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith('First λ, then 🐈'))
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
    const text = `\`\`\`text\n${code}\`\`\`\n\nRetained tail.`
    const { container } = render(<MessageContent text={text} label="Long answer" format="markdown" />)
    expect(screen.getByRole('button', { name: 'Expand Long answer' })).toHaveAttribute('aria-expanded', 'false')
    expect(container.querySelector('pre code')?.textContent).toBe(code)
    expect(screen.getByText(/Reading view is collapsed/)).toHaveTextContent('Copy retains the complete text.')
    fireEvent.click(screen.getByRole('button', { name: 'Expand Long answer' }))
    expect(screen.getByRole('button', { name: 'Collapse Long answer' })).toHaveAttribute('aria-expanded', 'true')
    expect(container.querySelector('.message-content-expanded')).toBeInTheDocument()
    expect(container.querySelector('pre code')?.textContent).toBe(code)
    fireEvent.click(screen.getByRole('button', { name: 'Copy Long answer' }))
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(text))
    fireEvent.click(screen.getByRole('button', { name: 'Collapse Long answer' }))
    expect(screen.getByRole('button', { name: 'Expand Long answer' })).toHaveAttribute('aria-expanded', 'false')
    expect(container.querySelector('pre code')?.textContent).toBe(code)
  })

  it('expands the viewport when a contained link receives keyboard focus', () => {
    overflowingViewport()
    render(<MessageContent text="[Retained link](https://example.com)" label="Long answer" format="markdown" />)
    fireEvent.focus(screen.getByRole('link', { name: 'Retained link' }))
    expect(screen.getByRole('button', { name: 'Collapse Long answer' })).toHaveAttribute('aria-expanded', 'true')
  })

  it('keeps short content free of expansion controls and permits copy-free details', () => {
    render(<MessageContent text="Short" label="Detail" copy={false} />)
    expect(screen.getByLabelText('Detail').textContent).toBe('Short')
    expect(screen.queryByRole('button')).toBeNull()
  })

  it('distinguishes role, status, folded body and independently expandable metadata', () => {
    const { container } = render(<MessagePresentation kind="reasoning" title="Reasoning summary" status="Retained" collapsed details={<p>Exact reference</p>}>
      <MessageContent text="Readable summary" label="Summary" />
    </MessagePresentation>)
    expect(screen.getByRole('listitem')).toHaveClass('chat-message-reasoning')
    expect(screen.getByText('Retained')).toBeVisible()
    expect(screen.getByLabelText('Summary')).not.toBeVisible()
    expect(screen.getByText('Exact reference')).not.toBeVisible()
    fireEvent.click(screen.getByText('Reasoning summary'))
    expect(container.querySelector('.chat-message-fold')).toHaveAttribute('open')
    expect(screen.getByLabelText('Summary')).toBeVisible()
    expect(screen.getByText('Exact reference')).not.toBeVisible()
    fireEvent.click(screen.getByText('Message details'))
    expect(screen.getByText('Exact reference')).toBeVisible()
  })
})
