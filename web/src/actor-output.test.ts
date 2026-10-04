import { describe, expect, it } from 'vitest'
import { actorOutputKey, isStoredActorOutput, decodeActorOutputPage } from './actor-output'
import { applyStateEvent, isSequencedEvent, normalizeSnapshot } from './protocol'
import { toViewModel } from './integration'

describe('actor authored output', () => {
  const origin = { run: 'run', nativeActor: '2', incarnation: '3' }
  const output = { reference: { origin, sequence: '5' }, emission: { origin,
    id: { displaySlot: '1', pageOrdinal: '1' }, page: { text: 'value', expansions: [['1', 'field']] as [string, string][], unavailable: false } } }
  const snapshot = { seq: '0', conversations: [], requests: [], jobs: [], envelopes: [], actorOutputRevisions: [output.reference] }
  it('retains canonical output identity without creating a provider exchange', () => {
    const restored = normalizeSnapshot(snapshot)
    expect(restored.actorOutputRevisions?.get(actorOutputKey(origin))).toEqual(output.reference)
    const changed = applyStateEvent(restored, { seq: '1', event: { kind: 'actor.output.committed', value: { ...output, reference: { origin, sequence: '6' } } } })
    expect(changed.kind).toBe('applied')
    if (changed.kind !== 'applied') return
    expect(toViewModel(changed.state).actorOutputRevisions?.[0]?.sequence).toBe('6')
    expect(changed.state.requests.size).toBe(0)
    expect(changed.state.conversations.size).toBe(0)
  })
  it('rejects mismatched authority and encoded metadata beyond its bound', () => {
    expect(isStoredActorOutput(output)).toBe(true)
    expect(isSequencedEvent({ seq: '1', event: { kind: 'actor.output.committed', value: { ...output, reference: { origin: { ...origin, incarnation: '4' }, sequence: '5' } } } })).toBe(false)
    expect(isStoredActorOutput({ ...output, emission: { ...output.emission, page: { ...output.emission.page, expansions: [['1', '\u0000'.repeat(1400)]] } } })).toBe(false)
  })
  it('reads full native actor and display identities without rounding its history cursor', () => {
    const exact = { run: 'run', nativeActor: '18446744073709551615', incarnation: '9007199254740993' }
    const retained = { reference: { origin: exact, sequence: '9223372036854775807' }, emission: { origin: exact,
      id: { displaySlot: '18446744073709551615', pageOrdinal: '9007199254740993' },
      page: { text: 'value', expansions: [['18446744073709551615', 'detail']], unavailable: false } } }
    const page = { origin: exact, outputs: [retained], nextAfter: '9223372036854775807' }
    expect(decodeActorOutputPage(page, exact, '9223372036854775806')).toEqual(page)
    expect(() => decodeActorOutputPage({ ...page, nextAfter: '9223372036854775806' }, exact, '9223372036854775806')).toThrow()
    expect(() => decodeActorOutputPage({ ...page, origin: { ...exact, incarnation: '9007199254740992' } }, exact, '0')).toThrow()
  })

})
