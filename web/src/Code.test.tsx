import { render } from '@testing-library/react'
import { expect, it } from 'vitest'
import Code from './Code'

it('highlights Haskell without changing Unicode, newlines, indentation, or literal escapes', () => {
  const text = '-- λ\r\nlet prompt = "literal \\n <img src=x onerror=alert(1)>"\n\n  observe (42 :: Int)\n'
  const { container, rerender } = render(<pre><Code text={text} language="haskell" /></pre>)
  expect(container.textContent).toBe(text)
  for (const token of ['keyword', 'string', 'comment', 'number']) expect(container.querySelector(`.token.${token}`)).not.toBeNull()
  expect(container.querySelector('img')).toBeNull()
  expect(container.querySelector('script')).toBeNull()
  const partial = 'let prompt = "unfinished λ'
  rerender(<pre><Code text={partial} language="haskell" /></pre>)
  expect(container.textContent).toBe(partial)
})
