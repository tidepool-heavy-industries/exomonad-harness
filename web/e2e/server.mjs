import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { resolve, extname, sep } from 'node:path';
import { snapshot, history } from './fixtures.mjs';

const root = resolve(process.env.HARNESS_BROWSER_ASSETS ?? 'dist');
const port = Number(process.env.HARNESS_BROWSER_PORT ?? 4387);
const sockets = new Set();
let config, observations, statuses;
function reset(value = {}) {
  for (const socket of sockets) socket.destroy();
  config = { authenticated: true, snapshot: snapshot(), receipt: 'refused', historyUnavailable: false, historyOversized: false, holdSnapshot: false, ...value };
  observations = { commands: [], snapshotRequests: 0, connections: 0, historyReads: [], statusReads: [] };
  statuses = new Map();
}
reset();
function json(response, value, status = 200) { response.writeHead(status, {'Content-Type':'application/json', 'Cache-Control':'no-store'}); response.end(JSON.stringify(value)); }
function frame(socket, value, opcode = 1) {
  const data = Buffer.from(typeof value === 'string' ? value : JSON.stringify(value));
  let head;
  if (data.length < 126) head = Buffer.from([0x80 | opcode, data.length]);
  else if (data.length < 65536) { head = Buffer.alloc(4); head[0] = 0x80 | opcode; head[1] = 126; head.writeUInt16BE(data.length, 2); }
  else { head = Buffer.alloc(10); head[0] = 0x80 | opcode; head[1] = 127; head.writeBigUInt64BE(BigInt(data.length), 2); }
  if (!socket.destroyed) socket.write(Buffer.concat([head, data]));
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
    statuses.set(id, { operationId: id, command, state: receipt?.outcome === 'admitted' ? 'input_admitted' : receipt?.outcome ?? 'queued', envelopeId: receipt?.outcome === 'admitted' ? 1 : null, receipt });
    frame(socket, {type:'command.accepted',command_id:id});
    if (receipt) {
      config.snapshot.seq++;
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
      if (url.pathname === '/__fixture/disconnect') for (const socket of sockets) socket.destroy();
      if (url.pathname === '/__fixture/snapshot') { config.snapshot = body; broadcast({type:'snapshot',snapshot:body}); }
      return json(response, observations);
    }
    if (url.pathname === '/api/session') {
      if (request.method === 'DELETE') config.authenticated = false;
      if (request.method === 'POST') config.authenticated = true;
      return json(response, {authenticated:config.authenticated});
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
      if (config.historyOversized && offset === 50) { result.items = []; result.nextOffset = null; result.oversizedItem = {position:50,hash:createHash('sha256').update('oversized-fixture').digest('hex'),byteLen:3000000,skipOffset:51}; }
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
server.on('upgrade', (request, socket) => {
  if (request.url !== '/api/ws' || !config.authenticated) return socket.destroy();
  const key = request.headers['sec-websocket-key'];
  const accept = createHash('sha1').update(key+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
  sockets.add(socket); observations.connections++;
  let pending = Buffer.alloc(0);
  socket.on('data', chunk => {
    pending = Buffer.concat([pending,chunk]);
    while (pending.length >= 2) {
      const opcode=pending[0]&15, masked=(pending[1]&128)!==0;
      let length=pending[1]&127, at=2;
      if(length===126){if(pending.length<4)return;length=pending.readUInt16BE(2);at=4;}
      if(length===127){if(pending.length<10)return;length=Number(pending.readBigUInt64BE(2));at=10;}
      if(length>1024*1024)return socket.destroy();
      if(pending.length<at+(masked?4:0)+length)return;
      const mask=masked?pending.subarray(at,at+4):null;at+=masked?4:0;
      const data=Buffer.from(pending.subarray(at,at+length));pending=pending.subarray(at+length);
      if(mask)for(let n=0;n<data.length;n++)data[n]^=mask[n%4];
      if(opcode===8){socket.end();return;}
      if(opcode===9){frame(socket,data.toString(),10);continue;}
      if(opcode===1)try{receive(socket,JSON.parse(data.toString()));}catch{socket.destroy();}
    }
  });
  socket.on('close',()=>sockets.delete(socket)); socket.on('error',()=>sockets.delete(socket));
  if(!config.holdSnapshot) frame(socket,{type:'snapshot',snapshot:config.snapshot});
});
server.listen(port,'127.0.0.1',()=>console.log(`Synthetic production-assets fixture transport http://127.0.0.1:${port}`));
for(const signal of ['SIGINT','SIGTERM'])process.on(signal,()=>{for(const socket of sockets)socket.destroy();server.close(()=>process.exit());});
