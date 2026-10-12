import { expect, type Page, type APIRequestContext } from '@playwright/test'
import type { ChatEntry, ChatTool } from '../src/generated/chat'
import type { StoredActorForm } from '../src/generated/actor-form'
import { snapshot, target } from './fixtures.mjs'
import { chooseWorker, openFixture } from './support'

export const origin = { run: target.run, nativeActor: '7', incarnation: '1' }
export const modelOrigin = { kind: 'embedded' as const, ...target }
const hash = 'a'.repeat(64)
export function actorSnapshot() {
  const data = snapshot()
  data.actors = data.actors.map((actor, index) => index === 1 ? { ...actor, outputOrigin: origin, modelHeadRequest: 'request-worker-1' } : actor)
  return data
}
export const message = (sequence: string, text: string, id: string | null = null): ChatEntry => ({ sequence, kind: 'message', requestId: 'request-worker-1', position: sequence,
  hash, tool: null, providerItemId: id, inputOperation: null, item: { type: 'message', role: 'assistant', content: text, ...(id ? { id } : {}) } })
export const form: StoredActorForm = { sequence: '5', revisionSequence: '5', state: 'open', attemptId: null, draft: null, errors: [], answer: null,
  opening: { origin, mountId: 'choose-validation', conversation: null, execution: { kind: 'actor_program' }, form: { version: 1, root: { kind: 'group', children: [
    { kind: 'section', title: 'Choose the next verification', child: { kind: 'text', id: 'project', label: 'Project name', initial: null } },
    { kind: 'choice', id: 'lane', label: 'Verification lane', initial: 'focused', options: [
      { id: 'focused', label: 'Focused browser checks', presentation: { kind: 'text', text: 'Focused browser checks — streaming, forms, and recovery' } },
      { id: 'full', label: 'Complete browser suite', presentation: { kind: 'text', text: 'Complete browser suite — all existing interactions' } },
    ] },
  ] } } } }
export const tool: ChatTool = { operation: { origin: modelOrigin, request: 'issuer', call: 'cell' }, invocation: { request: 'issuer', position: '0', hash } }
export const code = ['let checks =', '      [ "streaming selection"', '      , "retained forms"', '      , "reconnect continuity"', '      ]', ...Array.from({ length: 24 }, (_, index) => `-- Verification ${index + 1}: retain exact actor ownership across updates.`), 'traverse_ runCheck checks'].join('\n')
export const entries: ChatEntry[] = [
  message('1', '# Verification report\n\n' + Array.from({ length: 12 }, (_, index) => `Step ${index + 1} preserves the conversation while execution continues. You can read earlier results, select source text, and answer a pending form without losing your place.`).join('\n\n') + '\n\nThe complete report ends here.'),
  { sequence: '2', kind: 'message', requestId: 'issuer', position: '0', hash, tool, providerItemId: null, inputOperation: null, item: { type: 'custom_tool_call', call_id: 'cell', name: 'haskell', input: code } },
  { sequence: '3', kind: 'message', requestId: 'result', position: '0', hash, tool, providerItemId: null, inputOperation: null, item: { type: 'custom_tool_call_output', call_id: 'cell', output: 'Compilation failed\nCell.hs:31:11: Variable not in scope: runCheck\nNo effects were evaluated.' } },
  message('4', 'The compiler could not resolve `runCheck`. Choose a verification lane below while I prepare the corrected cell.'),
  { sequence: '5', kind: 'form', form },
]

export async function openChat(page: Page, request: APIRequestContext, chatEntries: ChatEntry[] = entries) {
  await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries })
  await chooseWorker(page)
  await expect(page.getByRole('navigation', { name: 'Pending responses' })).toBeVisible()
}

