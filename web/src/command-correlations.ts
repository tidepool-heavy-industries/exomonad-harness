import { canonicalOperationId, type HostCommandSubmission } from './protocol'
import type { SendObservation } from './ws-client'

/** Transient payloads needed to correlate a socket reply with a retained operation. */
export class CommandCorrelations {
  private readonly pending = new Map<string, HostCommandSubmission>()

  sent(submission: HostCommandSubmission, observation: SendObservation): void {
    if (observation !== 'not_sent') {
      this.pending.set(canonicalOperationId(submission.operation_id), submission)
    }
  }

  accepted(commandId: string): HostCommandSubmission | undefined {
    return this.take(commandId)
  }

  refused(commandId: string): HostCommandSubmission | undefined {
    return this.take(commandId)
  }

  private take(commandId: string): HostCommandSubmission | undefined {
    const key = canonicalOperationId(commandId)
    const submission = this.pending.get(key)
    this.pending.delete(key)
    return submission
  }
}
