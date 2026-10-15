import type { TimelineRow } from './chat-timeline'

export type ConversationGroup =
  | { kind: 'row'; key: string; row: TimelineRow }
  | { kind: 'tools'; key: string; rows: readonly TimelineRow[] }

export function isToolRow(row: TimelineRow): boolean {
  return row.kind === 'activity' || row.kind === 'result'
    || row.kind === 'live' && row.live.every(item => item.channel === 'tool_input' || item.channel === 'tool_arguments')
}

/** Disclosures change presentation only; original row and operation identities stay intact. */
export function groupToolSequences(rows: readonly TimelineRow[]): ConversationGroup[] {
  const groups: ConversationGroup[] = []
  let tools: TimelineRow[] | undefined
  for (const row of rows) {
    if (isToolRow(row)) {
      if (!tools) {
        tools = []
        groups.push({ kind: 'tools', key: JSON.stringify(['tools', row.key]), rows: tools })
      }
      tools.push(row)
    } else {
      tools = undefined
      groups.push({ kind: 'row', key: row.key, row })
    }
  }
  return groups
}
