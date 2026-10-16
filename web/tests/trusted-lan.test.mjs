import test from 'node:test';import assert from 'node:assert/strict';import {createHmac,webcrypto} from 'node:crypto';
import {hmacSHA256,randomUUID} from '../dist/crypto.js';import {SoundroomClient,authenticationProof} from '../dist/sdk.js';import {allowedHosts,approvedRequest,trustedAddress,upgradeRequest} from '../tools/trusted-lan.mjs';
if(!globalThis.crypto)Object.defineProperty(globalThis,'crypto',{value:webcrypto,configurable:true});
if(!globalThis.CustomEvent)globalThis.CustomEvent=class extends Event{constructor(type,options){super(type);this.detail=options?.detail;}};
test('HTTP-compatible HMAC matches SHA256 reference including long keys',()=>{
 for(const length of [0,20,32,64,131])for(const text of ['', 'Soundroom authentication', 'x'.repeat(500)]){const key=Buffer.alloc(length,0xab),message=Buffer.from(text);assert.equal(Buffer.from(hmacSHA256(key,message)).toString('hex'),createHmac('sha256',key).update(message).digest('hex'));}
});
test('pairing proof and IDs work without secure-context crypto APIs',async()=>{
 const old=Object.getOwnPropertyDescriptor(globalThis,'crypto');Object.defineProperty(globalThis,'crypto',{value:{getRandomValues:bytes=>webcrypto.getRandomValues(bytes)},configurable:true});
 try{assert.match(randomUUID(),/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);const c={serverID:'server',clientID:'client',clientSecret:Buffer.alloc(32,5).toString('base64')},peer={id:'runtime',name:'Browser'},nonce=Buffer.alloc(32,9).toString('base64');assert.equal(await authenticationProof(c,peer,nonce),createHmac('sha256',Buffer.alloc(32,5)).update(`soundroom-auth-v2|SERVER|CLIENT|RUNTIME|Browser|${nonce}`).digest('base64'));new SoundroomClient();}finally{Object.defineProperty(globalThis,'crypto',old);}
});
test('plaintext endpoints require explicit same-origin trusted LAN mode',async()=>{
 const socket={readyState:1,close(){},bufferedAmount:0},client=new SoundroomClient(()=>socket),code={code:'A'.repeat(43)};
 await assert.rejects(client.connect('ws://192.168.1.10:5174/control',code));client.allowTrustedLAN('http://192.168.1.10:5174');
 for(const url of ['ws://other.local:5174/control','ws://192.168.1.10:5175/control','ws://192.168.1.10:5174/','ws://192.168.1.10:5174/control?secret=x','ws://name@192.168.1.10:5174/control'])await assert.rejects(client.connect(url,code));
 await client.connect('ws://192.168.1.10:5174/control',code);client.disconnect();
});
test('gateway restricts host, origin, route and subprotocol',()=>{
 const hosts=allowedHosts(5174,{en0:[{family:'IPv4',address:'192.168.1.10'}]},'studio.local'),req={socket:{remoteAddress:'192.168.1.20'},method:'GET',url:'/control',headers:{host:'192.168.1.10:5174',origin:'http://192.168.1.10:5174',upgrade:'websocket','sec-websocket-protocol':'soundroom.v3','sec-websocket-version':'13','sec-websocket-key':'dGhlIHNhbXBsZSBub25jZQ=='}};
 assert.equal(approvedRequest(req,hosts,true),true);for(const change of [{host:'attacker.example:5174'},{origin:'http://evil.example'},{origin:undefined},{'sec-websocket-protocol':'other'}])assert.equal(approvedRequest({...req,headers:{...req.headers,...change}},hosts,true),false);assert.equal(approvedRequest({...req,url:'/elsewhere'},hosts,true),false);
});

test('trusted LAN rejects public peers and public interface addresses',()=>{
 for(const ip of ['127.0.0.1','10.0.1.2','172.16.0.1','172.31.255.255','192.168.1.2','169.254.1.2','::ffff:192.168.1.2'])assert.equal(trustedAddress(ip),true);
 for(const ip of ['8.8.8.8','172.32.0.1','192.169.0.1','::1','garbage',''])assert.equal(trustedAddress(ip),false);
 assert.equal(allowedHosts(5174,{en0:[{family:'IPv4',address:'8.8.8.8'}]},'studio.local').has('8.8.8.8:5174'),false);
 assert.equal(approvedRequest({socket:{remoteAddress:'8.8.8.8'},headers:{host:'localhost:5174'}},allowedHosts(5174)),false);
});

test('gateway builds a minimal canonical upgrade without caller body headers',()=>{
 const wire=upgradeRequest({'sec-websocket-key':'dGhlIHNhbXBsZSBub25jZQ==','content-length':'0','origin':'http://evil'},49160);
 assert.ok(wire.startsWith('GET /control HTTP/1.1\r\n'));assert.ok(wire.endsWith('\r\n\r\n'));
 assert.ok(wire.includes('Origin: https://localhost\r\n'));assert.ok(wire.includes('Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n'));assert.ok(!wire.includes('content-length'));assert.ok(!wire.includes('http://evil'));
});

test('pre-authentication network failures stop instead of reconnecting or replaying a code',async()=>{
 const socket={readyState:1,close(){this.onclose?.({});},bufferedAmount:0},client=new SoundroomClient(()=>socket),states=[];client.addEventListener('connection',e=>states.push(e.detail));
 await client.connect('wss://studio.local/control',{code:'A'.repeat(43)});socket.onerror({});socket.onclose({});
 assert.equal(client.state,'failed');assert.ok(states.at(-1).message.includes('Connection failed'));assert.ok(!states.some(s=>s.state==='reconnecting'));client.disconnect();
});
test('authentication rejection remains visible after socket closes',async()=>{
 const socket={readyState:1,send(){},close(){this.onclose?.({});},bufferedAmount:0},client=new SoundroomClient(()=>socket);let latest;client.addEventListener('connection',e=>latest=e.detail);
 await client.connect('wss://studio.local/control',{code:'A'.repeat(43)});socket.onmessage({data:JSON.stringify({version:3,kind:'failure',message:'Pairing code is expired'})});await new Promise(r=>setTimeout(r,0));
 assert.equal(client.state,'failed');assert.equal(latest.message,'Pairing code is expired');client.disconnect();
});

test('remembered pairing survives reload but stays scoped and can be forgotten',async()=>{
 const {loadPairing,savePairing,forgetPairing}=await import('../dist/remembered-pairing.js');const items=new Map(),storage={getItem:k=>items.get(k)??null,setItem:(k,v)=>items.set(k,v),removeItem:k=>items.delete(k)};
 const credential={version:3,serverID:'11111111-1111-4111-8111-111111111111',credentialID:'22222222-2222-4222-8222-222222222222',clientSecret:Buffer.alloc(32,5).toString('base64'),role:'editor'};
 savePairing(storage,{endpoint:'ws://192.168.1.10:5174/control',name:'Browser',credential});assert.equal(loadPairing(storage,'http://192.168.1.10:5174').credential.clientSecret,credential.clientSecret);
 assert.equal(loadPairing(storage,'http://other.local:5174'),undefined);assert.equal(items.size,0);
 savePairing(storage,{endpoint:'ws://localhost:5174/control',name:'Browser',credential});forgetPairing(storage);assert.equal(loadPairing(storage,'http://localhost:5174'),undefined);
});

test('pairing input pattern is valid under HTML Unicode Sets semantics',async()=>{
 const {readFile}=await import('node:fs/promises');const html=await readFile(new URL('../index.html',import.meta.url),'utf8');const pattern=html.match(/id="pairing-code"[^>]*pattern="([^"]+)"/)[1];
 let flag='u';try{new RegExp('','v');flag='v';}catch{}
 const regex=new RegExp(`^(?:${pattern})$`,flag);
 assert.equal(regex.test('A'.repeat(42)+'-'),true);assert.equal(regex.test('A'.repeat(42)+'_'),true);assert.equal(regex.test('A'.repeat(42)+'!'),false);assert.equal(regex.test('A'.repeat(42)),false);
});
test('an established session keeps retrying when a retry socket is refused before authentication',async(t)=>{
 t.mock.timers.enable({apis:['setTimeout']});
 const sockets=[],client=new SoundroomClient(()=>{const s={readyState:1,send(){},close(){},bufferedAmount:0};sockets.push(s);return s;}),states=[];client.addEventListener('connection',e=>states.push(e.detail.state));
 await client.connect('wss://studio.local/control',{code:'A'.repeat(43)});
 client.handshake='complete';sockets[0].onclose({});assert.equal(client.state,'reconnecting');
 t.mock.timers.tick(500);await new Promise(r=>setImmediate(r));assert.equal(sockets.length,2);
 sockets[1].onerror({});sockets[1].onclose({});assert.equal(client.state,'reconnecting','A refused retry spends budget instead of failing at once');
 t.mock.timers.tick(1000);await new Promise(r=>setImmediate(r));assert.equal(sockets.length,3);
 sockets[2].onerror({});sockets[2].onclose({});t.mock.timers.tick(2000);await new Promise(r=>setImmediate(r));
 sockets[3].onerror({});sockets[3].onclose({});assert.equal(client.state,'failed','Budget is bounded');assert.ok(!states.slice(0,-1).includes('failed'));
 client.disconnect();
});
