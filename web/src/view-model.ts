import type { CommandReceipt, Envelope, HostActorIdentity, RequestFailure } from "./protocol";

/**
 * Presentation rows derived from normalized server state. The integration
 * adapter owns projection; command receipts preserve their wire outcome union.
 */
export type HarnessViewModel = {
  liveOutput?: readonly import('./live-output').LiveOutput[];
  historyRevisions?: readonly import('./live-output').HistoryRevision[];
  actorOutputRevisions?: readonly import('./actor-output').ActorOutputReference[];
  hostRun?: string;
  actors?: Array<{
    id: string;
    name: string;
    run: string;
    incarnation: string;
    outputOrigin?: import('./actor-output').ActorOutputOrigin;
    parent?: string;
    parentIdentity?: HostActorIdentity | null;
    kind: "model" | "workflow";
    lifecycle: string;
    modelConversation?: string;
    modelHeadRequest?: string;
    activeRound?: string;
  }>;
  commandReceipts?: CommandReceipt[];
  nodes: Array<{
    id: string;
    parentId?: string | null;
    forkSourceRequestId?: string | null;
    version?: number;
    name: string;
    model?: string;
    effort?: string;
    state: string;
    detail?: string;
    updatedAt?: string;
  }>;
  timeline: Array<{
    id: string;
    /** Composite request/job identity for rendering; id retains the wire ID. */
    key?: string;
    nodeId: string;
    parentId?: string | null;
    label: string;
    kind: "request" | "job" | "wait";
    state: string;
    startedAt?: string;
    duration?: string;
    startedAtMs?: number;
    endedAtMs?: number;
    version?: number;
    historyRefreshKey?: string;
    detail?: string;
    commandId?: string;
    failure?: RequestFailure | null;
    command?: string;
    toolKind?: "function" | "custom";
    outcome?: "accepted" | "pending" | "queued" | "presented" | "acted" | "completed" | "cancelled" | "failed";
    requestId?: string;
    callId?: string;
    toolName?: string;
    delivered?: boolean;
    output?: unknown;
  }>;
  inbox: Array<{
    id: string;
    sender: string;
    recipient?: string;
    message: string;
    state: string;
    type?: Envelope["type"];
    receivedAt?: string;
    ordinal?: number;
  }>;
};
