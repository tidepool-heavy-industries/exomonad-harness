import { useMemo, type ReactNode } from 'react'
import { refractor } from 'refractor/core'
import haskell from 'refractor/haskell'
import './Code.css'

refractor.register(haskell)

export type CodeLanguage = 'haskell'
type Token = ReturnType<typeof refractor.highlight>['children'][number]

function renderTokens(tokens: readonly Token[]): ReactNode {
  return tokens.map((token, index) => {
    if (token.type === 'text') return token.value
    if (token.type !== 'element') return null
    const classes = token.properties.className
    // Only token classes and text enter React; grammar output never becomes HTML.
    return <span key={index} className={Array.isArray(classes) ? classes.join(' ') : undefined}>{renderTokens(token.children)}</span>
  })
}

export default function Code({ text, language }: { text: string; language: CodeLanguage }) {
  const highlighted = useMemo(() => renderTokens(refractor.highlight(text, language).children), [text, language])
  return <code className={`code-source language-${language}`}>{highlighted}</code>
}
