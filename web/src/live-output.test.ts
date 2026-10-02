import { describe, expect, it } from 'vitest'
import { appendOutput, commitOutput, belongsTo, outputKey, visibleOutput, type OutputUpdate } from './live-output'
import { applyStateEvent, isSequencedEvent, isSnapshot, normalizeSnapshot } from './protocol'
import { toViewModel } from './integration'

const origin = {kind:'embedded' as const, run:'run', actor:'/root', incarnation:'first'}
const delta: OutputUpdate = {origin, requestId:'pending', itemId:'assistant', channel:'assistant', index:0, text:'Hello ', overflow:false, version:1}
const base = {seq:0, hostRun:'run', conversations:[], requests:[], jobs:[], envelopes:[], actors:[{
  identity:origin, parent:null, kind:'model' as const, lifecycle:'running' as const, modelConversation:'/root', modelHeadRequest:'settled'}]}

describe('live output and retained history handoff', () => {
  it('separates exact actors, requests, items and channels, then reconciles a committed hash once', () => {
    let outputs = appendOutput(undefined, delta)
    outputs = appendOutput(outputs, {...delta, text:'world', version:2})
    outputs = appendOutput(outputs, {...delta, itemId:'other', text:'other', version:3})
    outputs = appendOutput(outputs, {...delta, origin:{...origin, incarnation:'second'}, text:'successor', version:4})
    expect(outputs.get(outputKey(delta))?.text).toBe('Hello world')
    const own = [...outputs.values()].filter(item => belongsTo(item, origin))
    expect(own).toHaveLength(2)
    outputs = commitOutput(outputs, {origin, requestId:'pending', itemId:'assistant', hash:'durable', version:5})
    // HTTP still has the previous slice: keep the partial visible during the gap.
    expect(visibleOutput([...outputs.values()], new Map())).toHaveLength(3)
    const loaded = new Map([['pending', new Set(['durable'])]])
    expect(visibleOutput([...outputs.values()], loaded).map(item => item.text)).toEqual(['other', 'successor'])
    expect(visibleOutput([...outputs.values()], new Map([['other-request', new Set(['durable'])]]))).toHaveLength(3)
  })
  it('advances the exact observation head and refresh revision independently of whole-round completion', () => {
    const state = normalizeSnapshot(base)
    const started = {seq:1, event:{kind:'model.output.started' as const, value:{origin, requestId:'pending'}}}
    const after = applyStateEvent(state, started)
    expect(after.kind).toBe('applied')
    if (after.kind !== 'applied') throw new Error('event failed')
    expect(toViewModel(after.state).actors?.[0]?.modelHeadRequest).toBe('pending')
    const streamed = applyStateEvent(after.state, {seq:2, event:{kind:'model.output.delta', value:{...delta, version:2}}})
    if (streamed.kind !== 'applied') throw new Error('delta failed')
    const committed = applyStateEvent(streamed.state, {seq:3, event:{kind:'model.output.committed', value:{origin, requestId:'pending', itemId:'assistant', hash:'hash', version:3}}})
    if (committed.kind !== 'applied') throw new Error('commit failed')
    expect(toViewModel(committed.state).historyRevisions?.[0]?.version).toBe(3)
    expect(committed.state.requests.size).toBe(0)
  })
  it('stops incomplete partials on provider exit without affecting another exact issuer', () => {
    const state = normalizeSnapshot({...base, liveOutput:[...appendOutput(undefined, delta).values()]})
    const stopped = applyStateEvent(state, {seq:1, event:{kind:'model.output.stopped', value:{origin, requestId:'pending'}}})
    if (stopped.kind !== 'applied') throw new Error('stop failed')
    expect(stopped.state.liveOutput?.get(outputKey(delta))?.streaming).toBe(false)
    expect(stopped.state.liveOutput?.get(outputKey(delta))?.text).toBe('Hello ')
  })
  it('reconnects from retained partial state without replay and bounds preview overflow explicitly', () => {
    const outputs = appendOutput(undefined, {...delta, text:'λ'.repeat(40000)})
    const item = outputs.get(outputKey(delta))!
    expect(item.text).toBe('λ'.repeat(32768))
    expect(item.overflow).toBe(true)
    const snapshot = {...base, seq:8, liveOutput:[item], historyRevisions:[{origin, requestId:'pending', version:8}]}
    expect(isSnapshot(snapshot)).toBe(true)
    const restored = normalizeSnapshot(snapshot)
    expect(restored.liveOutput?.get(outputKey(delta))).toEqual(item)
    expect(applyStateEvent(restored, {seq:8, event:{kind:'model.output.delta', value:delta}}).kind).toBe('resync')
    expect(isSequencedEvent({seq:9,event:{kind:'model.output.delta',value:{...delta,itemId:undefined}}})).toBe(false)
    expect(isSnapshot({...snapshot, liveOutput:[{...item, origin:{...origin, incarnation:''}}]})).toBe(false)
  })
})
