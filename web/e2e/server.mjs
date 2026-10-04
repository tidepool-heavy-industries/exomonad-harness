import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { resolve, extname, sep } from 'node:path';
import { snapshot, history } from './fixtures.mjs';
import { WebSocket, WebSocketServer } from 'ws';

const root = resolve(process.env.HARNESS_BROWSER_ASSETS ?? 'dist');
const port = Number(process.env.HARNESS_BROWSER_PORT ?? 4387);
const sockets = new Set();
let config, observations, statuses;
function reset(value = {}) {
  for (const socket of sockets) socket.terminate();
  config = { authenticated: true, snapshot: snapshot(), receipt: 'refused', historyUnavailable: false, historyOversized: false, holdSnapshot: false, ...value };
  observations = { commands: [], snapshotRequests: 0, connections: 0, historyReads: [], statusReads: [] };
  statuses = new Map();
}
reset();
function json(response, value, status = 200) { response.writeHead(status, {'Content-Type':'application/json', 'Cache-Control':'no-store'}); response.end(JSON.stringify(value)); }
function frame(socket, value) {
  if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(value));
}
function broadcast(value) { for (const socket of sockets) frame(socket, value); }
function receive(socket, message) {
  if (message.type === 'snapshot.request') {
    observations.snapshotRequests++;
    if (!config.holdSnapshot) frame(socket, {type:'snapshot',snapshot:config.snapshot});
  } else if (message.type === 'host_command') {
    observations.commands.push(message);
    const {operation_id: id, command} = message;
    const receipt = config.receipt === 'none' ? null : config.receipt === 'admitted'
      ? {commandId:id,target:command.target,outcome:'admitted',envelopeId:'1'}
      : config.receipt === 'control_requested'
      ? {commandId:id,target:command.target,outcome:'control_requested',control:command.action}
      : {commandId:id,target:command.target,outcome:config.receipt,reason:'Deterministic fixture result'};
    statuses.set(id, { operationId: id, command, state: receipt?.outcome === 'admitted' ? 'input_admitted' : receipt?.outcome ?? 'queued', envelopeId: receipt?.outcome === 'admitted' ? '1' : null, receipt });
    frame(socket, {type:'command.accepted',command_id:id});
    if (receipt) {
      config.snapshot.seq = (BigInt(config.snapshot.seq) + 1n).toString();
      config.snapshot.commandReceipts.push(receipt);
      broadcast({type:'event',event:{seq:config.snapshot.seq,event:{kind:'command.receipt',value:receipt}}});
    }
  } else if (message.type === 'command') {
    observations.commands.push(message);
    frame(socket, {type:'command.refused',operation_id:null,code:'unavailable',reason:'Standalone fixture commands have no live provider.'});
  }
}
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, `http://127.0.0.1:${port}`);
    if (url.pathname.startsWith('/__fixture/')) {
      const chunks = []; for await (const chunk of request) chunks.push(chunk);
      const body = chunks.length ? JSON.parse(Buffer.concat(chunks)) : {};
      if (url.pathname === '/__fixture/reset') reset(body);
      if (url.pathname === '/__fixture/config') Object.assign(config, body);
      if (url.pathname === '/__fixture/status') statuses.set(body.operationId, body);
      if (url.pathname === '/__fixture/frame') broadcast(body);
      if (url.pathname === '/__fixture/disconnect') for (const socket of sockets) socket.terminate();
      if (url.pathname === '/__fixture/snapshot') { config.snapshot = body; broadcast({type:'snapshot',snapshot:body}); }
      return json(response, observations);
    }
    if (url.pathname === '/api/session') {
      if (request.method === 'DELETE') config.authenticated = false;
      if (request.method === 'POST') config.authenticated = true;
      return json(response, {authenticated:config.authenticated,authentication:'secret',available:true});
    }
    if (url.pathname.startsWith('/api/commands/')) {
      const id = decodeURIComponent(url.pathname.slice('/api/commands/'.length));
      observations.statusReads.push(id);
      const record = statuses.get(id);
      if (config.statusDelay) await new Promise(r => setTimeout(r, config.statusDelay));
      if (config.statusUnavailable) return json(response, {}, 503);
      return json(response, record ?? {}, record ? 200 : 404);
    }
    if (url.pathname.startsWith('/api/history/')) {
      const id = decodeURIComponent(url.pathname.slice('/api/history/'.length));
      const offset = Number(url.searchParams.get('offset') ?? 0);
      observations.historyReads.push({id,offset});
      if (config.historyDelay) await new Promise(r => setTimeout(r, config.historyDelay));
      if (config.historyStatus) return json(response, {}, config.historyStatus);
      if (config.historyUnavailable) return json(response, {error:'Fixture history unavailable'}, 503);
      const result = history(id, offset);
      if (config.historyOversized && offset === 50) { result.items = []; result.nextOffset = '50'; result.oversizedItem = {position:'50',hash:createHash('sha256').update('oversized-fixture').digest('hex'),byteLen:'3000000',skipOffset:'51'}; }
      return json(response, result, result.oversizedItem ? 413 : 200);
    }
    if (url.pathname.startsWith('/api/')) return json(response, {error:'Fixture route not found'}, 404);
    const file = resolve(root, `.${url.pathname}`);
    if (file !== root && !file.startsWith(root + sep)) return json(response, {}, 403);
    const path = extname(file) ? file : resolve(root,'index.html');
    const data = await readFile(path);
    response.writeHead(200, {'Content-Type':({'.html':'text/html','.js':'text/javascript','.css':'text/css'}[extname(path)] ?? 'application/octet-stream')}); response.end(data);
  } catch (error) { json(response,{error:String(error)},500); }
});
const websocketServer = new WebSocketServer({ noServer: true, maxPayload: 1024 * 1024 });
server.on('upgrade', (request, socket, head) => {
  if (request.url !== '/api/ws' || !config.authenticated) return socket.destroy();
  websocketServer.handleUpgrade(request, socket, head, (connection) => {
    sockets.add(connection);
    observations.connections++;
    connection.on('message', (data, isBinary) => {
      if (isBinary) return connection.close(1003, 'Fixture expects JSON text');
      try { receive(connection, JSON.parse(data.toString())); }
      catch { connection.close(1007, 'Invalid fixture JSON'); }
    });
    connection.on('close', () => sockets.delete(connection));
    connection.on('error', () => { sockets.delete(connection); connection.terminate(); });
    if (!config.holdSnapshot) frame(connection, { type: 'snapshot', snapshot: config.snapshot });
  });
});
server.listen(port,'127.0.0.1',()=>console.log(`Synthetic production-assets fixture transport http://127.0.0.1:${port}`));
for(const signal of ['SIGINT','SIGTERM'])process.on(signal,()=>{for(const socket of sockets)socket.terminate();websocketServer.close();server.close(()=>process.exit());});
