import { createContext, useContext, useRef, useState, type ReactNode } from 'react'

// Owned by a bounded timeline row, so preview → retained rendering can reuse
// choices without retaining a second copy of history or leaking across actors.
const Memory = createContext<Map<string, boolean> | undefined>(undefined)
export function PresentationMemory({ children }: { children: ReactNode }) {
  const values = useRef(new Map<string, boolean>())
  return <Memory.Provider value={values.current}>{children}</Memory.Provider>
}
export function usePresentationChoice(key: string, initial = false) {
  const memory = useContext(Memory)
  const [value, setValue] = useState(() => memory?.get(key) ?? initial)
  return [value, (next: boolean) => { memory?.set(key, next); setValue(next) }] as const
}
export function PresentationDetails({ memoryKey, summary, children, className }: {
  memoryKey: string; summary: ReactNode; children: ReactNode; className?: string
}) {
  const [open, setOpen] = usePresentationChoice(memoryKey)
  return <details className={className} open={open} onToggle={event => setOpen(event.currentTarget.open)}>
    <summary>{summary}</summary>{children}
  </details>
}
