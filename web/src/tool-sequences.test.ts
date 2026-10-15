import { expect, it } from 'vitest'
import { groupToolSequences } from './tool-sequences'
import type { TimelineRow, MessageEntry } from './chat-timeline'
import type { StoredActorForm } from './form-model'
const entry = (key: string, item: unknown): MessageEntry => ({ kind: 'message', sequence: key, requestId: 'r', position: '0', hash: 'a'.repeat(64), tool: null, providerItemId: null, inputOperation: null, item })
const message = (key: string): TimelineRow => ({ kind: 'entry', key, entry: entry(key, { type: 'message', role: 'assistant', content: 'prose' }) })
const tool = (key: string): TimelineRow => ({ kind: 'activity', key, entry: entry(key, { type: 'custom_tool_call', name: 'haskell', call_id: key, input: 'same source' }), results: [] })
const form: StoredActorForm = { sequence: '6', revisionSequence: '6', state: 'open', attemptId: null, draft: null, errors: [], answer: null,
  opening: { origin: { run: 'r', nativeActor: '1', incarnation: '1' }, mountId: 'question', conversation: null, execution: { kind: 'actor_program' },
    form: { version: 1, root: { kind: 'text', id: 'answer', label: 'Answer', initial: null } } } }
it('groups adjacent tool identities without crossing prose or forms or rewriting any row', () => {
  const formRow: TimelineRow = { kind: 'entry', key: 'form', entry: { kind: 'form', sequence: '6', form } }
  const rows = [message('1'), tool('2'), tool('3'), message('4'), tool('5'), formRow, tool('7')]
  const groups = groupToolSequences(rows)
  expect(groups.map(group => group.kind)).toEqual(['row', 'tools', 'row', 'tools', 'row', 'tools'])
  expect(groups.flatMap(group => group.kind === 'tools' ? group.rows : [group.row]).every((row, i) => row === rows[i])).toBe(true)
  expect(groupToolSequences([...rows, tool('8')]).at(-1)?.key).toBe(groups.at(-1)?.key)
})
