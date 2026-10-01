import { describe, expect, it } from 'vitest'
import { layoutActiveWorkers } from './active-worker-tree'
import type { HarnessViewModel } from './view-model'

type Actor = NonNullable<HarnessViewModel['actors']>[number]
const actor = (name: string, lifecycle = 'waiting', parentIdentity: Actor['parentIdentity'] = null, run = 'run'): Actor => ({
  id: JSON.stringify([run, name, 'incarnation-1']), name, run, incarnation: 'incarnation-1', parentIdentity,
  kind: 'model', lifecycle,
})
const identity = (name: string, run = 'run') => ({ run, actor: name, incarnation: 'incarnation-1' })

describe('layoutActiveWorkers', () => {
  it('uses exact parent identities and retains unavailable ancestors for active descendants', () => {
    const retiredParent = actor('/parent', 'retired')
    const activeChild = actor('/parent/child', 'running', identity('/parent'))
    const samePathOtherIncarnation = { ...actor('/parent', 'waiting'), id: 'replacement', incarnation: 'incarnation-2' }
    const otherRun = actor('/elsewhere', 'running', null, 'old-run')

    const layout = layoutActiveWorkers([retiredParent, activeChild, samePathOtherIncarnation, otherRun], 'run')
    expect(layout.nodes.map(node => node.actor.name)).toEqual(['/parent/child', '/parent', '/parent'])
    const child = layout.nodes.find(node => node.actor.name === '/parent/child')
    const oldParent = layout.nodes.find(node => node.actor.incarnation === 'incarnation-1' && node.context)
    expect(child?.parentKey).toBe(oldParent?.key)
    expect(oldParent?.context).toBe(true)
    expect(layout.nodes.find(node => node.actor.id === 'replacement')?.context).toBe(false)
    expect(layout.nodes.some(node => node.actor.run !== 'run')).toBe(false)
  })

  it('hides finished actors by default and includes them only when requested', () => {
    const rows = [actor('/live'), actor('/done', 'retired'), actor('/lost', 'lost')]
    expect(layoutActiveWorkers(rows, 'run').nodes.map(node => node.actor.name)).toEqual(['/live'])
    expect(layoutActiveWorkers(rows, 'run', true).nodes.map(node => node.actor.name)).toEqual(['/live', '/done', '/lost'])
  })

  it('marks missing parents and breaks cycles into visible roots', () => {
    const orphan = actor('/orphan', 'running', identity('/missing'))
    const first = actor('/first', 'running', identity('/second'))
    const second = actor('/second', 'running', identity('/first'))
    const layout = layoutActiveWorkers([orphan, first, second], 'run')
    expect(layout.nodes.find(node => node.actor.name === '/orphan')?.lineage).toBe('orphan')
    expect(layout.nodes.filter(node => node.lineage === 'cycle')).toHaveLength(1)
    expect(layout.nodes).toHaveLength(3)
    expect(layout.edges).toHaveLength(1)
  })

  it('bounds the rendered layout and reports omitted actors', () => {
    const rows = Array.from({ length: 900 }, (_, i) => actor(`/worker-${i}`))
    const layout = layoutActiveWorkers(rows, 'run', false, 100)
    expect(layout.nodes).toHaveLength(100)
    expect(layout.omitted).toBe(800)
    expect(layout.width).toBeLessThan(400)
  })
})
