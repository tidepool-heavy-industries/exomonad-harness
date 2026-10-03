import { createHash } from 'node:crypto';
/** Synthetic browser wire fixtures: they do not prove host admission or cleanup. */
export const target = { run: 'run-browser-fixture', actor: '/root/worker', incarnation: 'incarnation-one' };
export function snapshot(workerCount = 2, requestsPerWorker = 2) {
  const actors = [], conversations = [], requests = [], jobs = [], envelopes = [];
  for (let n = 0; n < workerCount; n++) {
    const id = n === 0 ? 'root' : n === 1 ? 'worker' : `worker-${n}`;
    const path = n === 0 ? '/root' : n === 1 ? '/root/worker' : `/root/worker-${n}`;
    const identity = n === 1 ? target : { ...target, actor: path, incarnation: `incarnation-${n}` };
    actors.push({ identity, parent: n === 0 ? null : actors[0].identity, kind: 'model', lifecycle: 'waiting', modelConversation: id, activeRound: '00000000-0000-4000-8000-000000000001' });
    conversations.push({ id, path, parentId: n === 0 ? null : 'root', state: 'paused', version: 1 });
    for (let r = 0; r < requestsPerWorker; r++) requests.push({ id: `request-${id}-${r}`, conversationId: id, state: 'completed', createdAtMs: 1700000000000 + n * 1000 + r, endedAtMs: 1700000000500 + n * 1000 + r, version: 1 });
    jobs.push({ id: `job-${id}`, conversationId: id, requestId: `request-${id}-0`, callId: `call-${id}`, toolKind: 'custom', toolName: 'run', state: 'cancelled', delivered: true, output: { status: 'cancelled', reason: 'fixture cancellation' }, version: 1 });
    envelopes.push({ id: `message-${id}`, sender: path, recipient: '/operator', type: 'MESSAGE', payload: `Fixture status for ${path}`, ordinal: n, version: 1 });
  }
  return { seq: 10, hostRun: target.run, actors, conversations, requests, jobs, envelopes, commandReceipts: [] };
}
export function history(requestId, offset = 0) {
  const items = Array.from({ length: 50 }, (_, n) => ({ position: offset + n, hash: createHash('sha256').update(`fixture-item-${offset+n}`).digest('hex'), byteLen: 100, item: n === 0 ? { type: 'message', role: 'assistant', content: [{type:'output_text', text:`Retained fixture message ${requestId} page ${offset}\n  preserved whitespace 🐚`}] } : n === 1 ? { type: 'future_unknown_item', fixture: `unknown ${offset+n}` } : n === 2 ? {type:'function_call', name:'read_file', call_id:'fixture-call', arguments:'{"path":"README"}'} : n === 3 ? {type:'function_call_output', call_id:'fixture-call', output:JSON.stringify({ready_results:[{call:'call-ready',origin:{actor:'/root',incarnation:'1',kind:'embedded',run:'run-1'},request:'request-1'}],reason:'tool_result'})} : {type:'message',role:'user',content:[{type:'input_text',text:`History item ${offset+n}`}]} }));
  return { requestId, parentId: null, branch: 'browser-fixture', items, nextOffset: offset < 100 ? offset + 50 : null, oversizedItem: null };
}
