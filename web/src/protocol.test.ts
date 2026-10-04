import { actorIdentityKey, applyStateEvent, isEmbeddedCommandRecord, isSequencedEvent, isSnapshot, normalizeSnapshot, type Snapshot } from './protocol'
import { fixtureSnapshot } from './fixture'
import { nextCounter } from './decimal'
import { describe, expect, it } from 'vitest'

describe('generated browser state projection', () => {
  it('applies exact consecutive counters above JavaScript safe integers without mutating prior state', () => {
    const snapshot: Snapshot = { ...fixtureSnapshot, seq: '9007199254740992' }
    expect(isSnapshot(snapshot)).toBe(true)
    const initial = normalizeSnapshot(snapshot)
    const message = { seq: '9007199254740993', event: { kind: 'job.upsert' as const,
      value: { id: 'job-next', conversationId: 'root', state: 'running' as const, startedAtMs: null, endedAtMs: null } } }
    expect(isSequencedEvent(message)).toBe(true)
    const applied = applyStateEvent(initial, message)
    if (applied.kind !== 'applied') throw new Error('valid consecutive event failed')
    expect(applied.state.seq).toBe('9007199254740993')
    expect(applied.state.jobs.has('job-next')).toBe(true)
    expect(initial.seq).toBe(snapshot.seq)
    expect(initial.jobs.has('job-next')).toBe(false)
    expect(applyStateEvent(initial, { ...message, seq: '9007199254740994' }))
      .toEqual({ kind: 'resync', expected: '9007199254740993', received: '9007199254740994' })
  })

  it('projects exact actor incarnations and their authoritative history heads across reconnect', () => {
    const identity = { run: 'run', actor: 'worker', incarnation: 'second' }
    const actor = { identity, parent: null, kind: 'model' as const, lifecycle: 'waiting' as const,
      modelConversation: 'root', modelHeadRequest: 'exact-head' }
    const state = normalizeSnapshot({ ...fixtureSnapshot, actors: [actor] })
    const message = { seq: nextCounter(state.seq), event: { kind: 'actor.upsert' as const,
      value: { ...actor, lifecycle: 'retired' as const } } }
    const result = applyStateEvent(state, message)
    if (result.kind !== 'applied') throw new Error('valid actor event failed')
    const key = actorIdentityKey(identity)
    expect(result.state.actors.get(key)?.modelHeadRequest).toBe('exact-head')
    expect(result.state.actors.get(key)?.lifecycle).toBe('retired')
    expect(normalizeSnapshot({ ...fixtureSnapshot, seq: message.seq, actors: [message.event.value] }).actors.get(key))
      .toEqual(result.state.actors.get(key))
    expect(state.actors.get(key)?.lifecycle).toBe('waiting')
  })

  it('bounds handoff receipts and moves a replacement to the newest position', () => {
    let state = normalizeSnapshot(fixtureSnapshot)
    for (let index = 0; index < 129; index++) {
      const result = applyStateEvent(state, { seq: nextCounter(state.seq), event: { kind: 'command.receipt',
        value: { commandId: `cmd-${index}`, outcome: 'refused', reason: 'not admitted' } } })
      if (result.kind !== 'applied') throw new Error('receipt event failed')
      state = result.state
    }
    expect(state.commandReceipts.size).toBe(128)
    expect([...state.commandReceipts.keys()][0]).toBe('cmd-1')
    const replacement = applyStateEvent(state, { seq: nextCounter(state.seq), event: { kind: 'command.receipt',
      value: { commandId: 'cmd-1', outcome: 'admitted', envelopeId: '9223372036854775807' } } })
    if (replacement.kind !== 'applied') throw new Error('replacement failed')
    expect(replacement.state.commandReceipts.size).toBe(128)
    expect([...replacement.state.commandReceipts.keys()].at(-1)).toBe('cmd-1')
    expect(replacement.state.commandReceipts.get('cmd-1')).toMatchObject({ envelopeId: '9223372036854775807' })
  })

  it('checks operation, target and receipt association after generated shape validation', () => {
    const target = { run: 'run', actor: '/worker', incarnation: 'one' }
    const operationId = 'ABCDEF01-1111-4111-8111-111111111111'
    const valid = { operationId: operationId.toLowerCase(), command: { action: 'input', target, text: 'hello' }, state: 'input_admitted',
      envelopeId: '9223372036854775807', receipt: { commandId: operationId.toLowerCase(), target,
        outcome: 'admitted', envelopeId: '9223372036854775807' } }
    expect(isEmbeddedCommandRecord(valid)).toBe(true)
    expect(isEmbeddedCommandRecord({ ...valid, envelopeId: '9223372036854775806' })).toBe(false)
    expect(isEmbeddedCommandRecord({ ...valid, receipt: { ...valid.receipt, target: { ...target, incarnation: 'two' } } })).toBe(false)
  })

  it('keeps provider diagnostic rendering bounded after generated shape validation', () => {
    const failure = { kind: 'http', status: 400, diagnostic: { code: '😀'.repeat(256), message: '😀'.repeat(2048) } }
    const request = { ...fixtureSnapshot.requests[0]!, failure }
    const snapshot = { ...fixtureSnapshot, requests: [request] }
    expect(isSnapshot(snapshot)).toBe(true)
    expect(isSequencedEvent({ seq: '1', event: { kind: 'request.upsert', value: request } })).toBe(true)
    for (const diagnostic of [{ code: '😀'.repeat(257) }, { message: '😀'.repeat(2049) }]) {
      const value = { ...request, failure: { ...failure, diagnostic } }
      expect(isSnapshot({ ...snapshot, requests: [value] })).toBe(false)
      expect(isSequencedEvent({ seq: '1', event: { kind: 'request.upsert', value } })).toBe(false)
    }
  })

  it('refuses undeclared auxiliary events at the generated boundary', () => {
    expect(isSequencedEvent({ seq: '1', event: { kind: 'tokens.observed', value: null } })).toBe(false)
  })
})
