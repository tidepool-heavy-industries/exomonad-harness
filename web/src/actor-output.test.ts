import { describe, expect, it } from 'vitest'
import { actorOutputKey, isStoredActorOutput } from './actor-output'
import { applyStateEvent, isSequencedEvent, normalizeSnapshot } from './protocol'
import { toViewModel } from './integration'

describe('actor authored output', () => {
  const origin = { run: 'run', nativeActor: 2, incarnation: 3 }
  const output = { reference: { origin, sequence: 5 }, emission: { origin,
    id: { displaySlot: 1, pageOrdinal: 1 }, page: { text: 'value', expansions: [[1, 'field']] as [number, string][], unavailable: false } } }
  const snapshot = { seq: 0, conversations: [], requests: [], jobs: [], envelopes: [], actorOutputRevisions: [output.reference] }
  it('retains canonical output identity without creating a provider exchange', () => {
    const restored = normalizeSnapshot(snapshot)
    expect(restored.actorOutputRevisions?.get(actorOutputKey(origin))).toEqual(output.reference)
    const changed = applyStateEvent(restored, { seq: 1, event: { kind: 'actor.output.committed', value: { ...output, reference: { origin, sequence: 6 } } } })
    expect(changed.kind).toBe('applied')
    if (changed.kind !== 'applied') return
    expect(toViewModel(changed.state).actorOutputRevisions?.[0]?.sequence).toBe(6)
    expect(changed.state.requests.size).toBe(0)
    expect(changed.state.conversations.size).toBe(0)
  })
  it('rejects mismatched authority and encoded metadata beyond its bound', () => {
    expect(isStoredActorOutput(output)).toBe(true)
    expect(isStoredActorOutput({ ...output, emission: { ...output.emission, page: { ...output.emission.page, view: { kind: 'text', text: 'value', truncated: true } } } })).toBe(true)
    expect(isStoredActorOutput({ ...output, emission: { ...output.emission, page: { ...output.emission.page, view: { kind: 'text', text: 'value', truncated: 'yes' } } } })).toBe(false)
    expect(isSequencedEvent({ seq: 1, event: { kind: 'actor.output.committed', value: { ...output, reference: { origin: { ...origin, incarnation: 4 }, sequence: 5 } } } })).toBe(false)
    expect(isStoredActorOutput({ ...output, emission: { ...output.emission, page: { ...output.emission.page, expansions: [[1, '\u0000'.repeat(1400)]] } } })).toBe(false)
  })
})
