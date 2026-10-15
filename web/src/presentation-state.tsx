import { createContext, useContext, useRef, useState, type ReactNode } from 'react'

// Owned by a bounded timeline row, so preview → retained rendering can reuse
// choices without retaining a second copy of history or leaking across actors.
const Memory = createContext<{ values: Map<string, boolean>; prefix: string } | undefined>(undefined)
export function PresentationMemory({ children, memory, namespace }: { children: ReactNode; memory?: Map<string, boolean>; namespace?: string }) {
  const values = useRef(new Map<string, boolean>())
  const parent = useContext(Memory)
  const value = namespace && parent ? { values: parent.values, prefix: `${parent.prefix}${JSON.stringify(namespace)}:` } : { values: memory ?? values.current, prefix: '' }
  return <Memory.Provider value={value}>{children}</Memory.Provider>
}
export function usePresentationChoice(key: string, initial = false) {
  const memory = useContext(Memory)
  const scopedKey = `${memory?.prefix ?? ''}${key}`
  const [value, setValue] = useState(() => memory?.values.get(scopedKey) ?? initial)
  return [value, (next: boolean) => { memory?.values.set(scopedKey, next); setValue(next) }] as const
}
export function PresentationDetails({ memoryKey, summary, children, className }: {
  memoryKey: string; summary: ReactNode; children: ReactNode; className?: string
}) {
  const [open, setOpen] = usePresentationChoice(memoryKey)
  return <details className={className} open={open} onToggle={event => setOpen(event.currentTarget.open)}>
    <summary>{summary}</summary>{children}
  </details>
}
