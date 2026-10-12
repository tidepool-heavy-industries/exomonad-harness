import { useEffect, useLayoutEffect, useRef, type RefObject } from 'react'
import { compareDecimal } from './decimal'
import type { StoredActorForm } from './form-model'

interface FormFocusIntent { attemptId: string; startRevision: string; responseReceived: boolean; submissionFocused: boolean }

export function useFormFocus(section: RefObject<HTMLElement>, latest: StoredActorForm, busy: boolean) {
  const focusIntent = useRef<FormFocusIntent>()
  // Submission acknowledgement can precede native validation. Keep focus intent
  // for that exact attempt until settlement or deliberate activity elsewhere.
  useEffect(() => {
    const abandonFocus = (event: Event) => {
      if (focusIntent.current && event.target instanceof Node && !section.current?.contains(event.target)) focusIntent.current = undefined
    }
    document.addEventListener('focusin', abandonFocus)
    document.addEventListener('pointerdown', abandonFocus)
    return () => { document.removeEventListener('focusin', abandonFocus); document.removeEventListener('pointerdown', abandonFocus) }
  }, [])
  useLayoutEffect(() => {
    const intent = focusIntent.current
    if (!intent || busy || !intent.responseReceived) return
    if (latest.attemptId !== intent.attemptId) {
      if (compareDecimal(latest.revisionSequence, intent.startRevision) > 0) focusIntent.current = undefined
      return
    }
    const focused = document.activeElement
    if (focused !== document.body && !section.current?.contains(focused)) { focusIntent.current = undefined; return }
    if (latest.state === 'open') {
      if (!latest.errors.length) return
      focusIntent.current = undefined
      const invalid = section.current?.querySelector<HTMLElement>('input[aria-invalid="true"], textarea[aria-invalid="true"], select[aria-invalid="true"]')
      if (invalid) invalid.focus()
      else section.current?.focus({ preventScroll: true })
    } else {
      if (!intent.submissionFocused) { intent.submissionFocused = true; section.current?.focus({ preventScroll: true }) }
      if (latest.state !== 'submitted') focusIntent.current = undefined
    }
  }, [latest, busy])
  return focusIntent
}
