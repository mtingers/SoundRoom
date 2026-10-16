// Live acceptance: unmodified browser SDK, mirror, Write controller, Touch
// pointer/keyboard bindings and draft worker against the real portable service
// over verified WSS. The host's callback is simulated; no audio device opens.
// Run with web/tests/live/run.sh (skips unless SOUNDROOM_LIVE_WRITE_HOST is set).
import test from 'node:test';import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';import {mkdtempSync,readFileSync,rmSync} from 'node:fs';import {tmpdir} from 'node:os';import {join} from 'node:path';
import {Worker} from 'node:worker_threads';import {createInterface} from 'node:readline';
import {SoundroomClient} from '../../dist/sdk.js';
import {SessionMirror,validateSnapshot,validateDelta,validateTransport} from '../../dist/model.js';
import {SnapshotAssembler} from '../../dist/snapshots.js';
import {AutomationWriteController,automationWriteAvailable} from '../../dist/automation-write.js';
import {AutomationTouchBindings,TouchControlBinding} from '../../dist/automation-touch.js';
import {WriteDraftWorker} from '../../dist/write-draft.js';
import {validateModel} from '../../dist/validate.js';
const host=process.env.SOUNDROOM_LIVE_WRITE_HOST;
const skip=host?false:'Set SOUNDROOM_LIVE_WRITE_HOST (see run.sh)';
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
let describe=()=>'';
async function until(predicate,label,ms=15000){const deadline=Date.now()+ms;while(!predicate()){if(Date.now()>deadline)throw Error(`Timed out: ${label} ${describe()}`);await sleep(5);}}
async function start(){const handles={};try{return await open(handles);}catch(error){handles.client?.disconnect();handles.child?.kill();throw error;}}
async function open(handles){
 const dir=mkdtempSync(join(tmpdir(),'soundroom-live-write-')),root=join(dir,'root'),credentialPath=join(dir,'credential.json');
 const child=spawn(host,[root,process.env.SOUNDROOM_TEST_TLS_P12,process.env.SOUNDROOM_LIVE_WRITE_PROJECT,credentialPath],{stdio:['pipe','pipe','inherit']});handles.child=child;
 const line=await new Promise((resolve,reject)=>{child.once('exit',code=>reject(Error(`host exited ${code}`)));createInterface({input:child.stdout}).once('line',resolve);});
 const {port}=JSON.parse(line),credential=JSON.parse(readFileSync(credentialPath,'utf8'));
 const sockets=[],sent=[];
 const client=new SoundroomClient(url=>{const socket=new WebSocket(url,{protocols:['soundroom.v3'],headers:{Origin:'https://localhost'}});const send=socket.send.bind(socket);socket.send=data=>{sent.push(String(data));send(data);};sockets.push(socket);return socket;});
 const mirror=new SessionMirror(),assembler=new SnapshotAssembler();let ready=false;
 const environment={available:()=>automationWriteAvailable(client.capabilities,client.role,client.state==='connected'&&ready),epoch:()=>client.connectionEpoch,serverID:()=>mirror.snapshot?.serverID,credentialID:()=>client.credentialIdentity,runtimeID:()=>client.clientID,version:id=>mirror.snapshot?.trackRevisions[id],track:id=>id===mirror.snapshot?.session.masterTrack.id?mirror.snapshot.session.masterTrack:mirror.snapshot?.session.tracks.find(t=>t.id===id),metadata:()=>client.capabilities.effects??[],playing:()=>!!mirror.snapshot?.transport.isPlaying,looping:()=>mirror.snapshot?.transport.loopStart!=null&&mirror.snapshot?.transport.loopEnd!=null,send:(command,versions={})=>client.command(command,undefined,versions),changed:()=>{}};
 const controller=new AutomationWriteController(environment),statuses=[],bindings=new AutomationTouchBindings(controller,message=>statuses.push(message));
 client.addEventListener('connection',e=>{if(e.detail.state==='failed')console.error('connection failed:',e.detail.message);if(e.detail.state!=='connected'){ready=false;controller.disconnected();}});
 client.addEventListener('snapshotpage',e=>{const m=e.detail;if(m.kind==='snapshotBegin'){assembler.begin(m);ready=false;}else if(m.kind==='snapshotPage')assembler.page(m);else{const s=assembler.end(m);validateModel(s,'RemoteSnapshot');assert.ok(validateSnapshot(s));mirror.applySnapshot(s,performance.now());ready=true;controller.metadataChanged();}});
 client.addEventListener('serverevent',e=>{const event=e.detail;if(event.type==='automationWrite')controller.update(event.update);else if(event.type==='delta'){assert.ok(validateDelta(event.delta));mirror.applyDelta(event.delta);controller.metadataChanged();}else if(event.type==='transport'&&validateTransport(event.transport))mirror.updateTransport(event.transport,performance.now());else if(event.type==='snapshot'&&validateSnapshot(event.snapshot)){mirror.applySnapshot(event.snapshot,performance.now());ready=true;controller.metadataChanged();}});
 handles.client=client;await client.connect(`wss://localhost:${port}/control`,credential,'Browser acceptance');
 await until(()=>client.state==='connected'&&ready,'authenticated snapshot');
 assert.equal(environment.available(),true,'host advertises live Write to an operator');
 const play=await client.command({type:'play'});assert.equal(play.status,'accepted');
 await until(()=>mirror.snapshot.transport.isPlaying,'callback-confirmed Play');
 const trackID=mirror.snapshot.session.tracks[0].id;describe=()=>JSON.stringify({current:controller.current,statuses:statuses.slice(-3)});
 return {child,dir,client,mirror,controller,bindings,statuses,sockets,sent,trackID,stop(){client.disconnect?.();child.stdin.end();child.kill();rmSync(dir,{recursive:true,force:true});}};
}
const volume={kind:'volume'};
const lane=h=>h.mirror.snapshot.session.tracks.find(t=>t.id===h.trackID).mixAutomation.volume??[];
const same=(a,b)=>JSON.stringify(a)===JSON.stringify(b);
const writeOps=h=>h.sent.map(f=>JSON.parse(f)).filter(f=>f.kind==='request'&&f.command.type==='editor'&&f.command.intent.operation.startsWith('automation.write')).map(f=>f.command.intent.operation);
function draftWorker(){const worker=new Worker(new URL('./draft-worker-shim.mjs',import.meta.url));const like={onmessage:null,onerror:null,postMessage:(value,transfer)=>worker.postMessage(value,transfer),terminate:()=>void worker.terminate()};worker.on('message',data=>like.onmessage?.({data}));worker.on('error',error=>like.onerror?.(error));return like;}

test('pointer and keyboard Touch gestures commit on the real host, Read stays static, Undo restores',{skip},async()=>{
 const h=await start();try{
  const original=structuredClone(lane(h));assert.equal(h.bindings.arm(h.trackID,volume,true),true);
  const control=new TouchControlBinding(h.bindings,h.trackID,volume);
  control.pointerDown();
  for(let i=1;i<=24;i++){assert.equal(control.input(0.3+i/100),true);await sleep(8);}
  control.pointerUp(0.6);
  await until(()=>h.controller.current?.phase==='committed','pointer commit with matching authored metadata');
  const committed=h.controller.current;assert.equal(h.mirror.snapshot.trackRevisions[h.trackID],committed.revision);
  const points=lane(h);assert.ok(points.length>=2,'authored lane');assert.ok(points.some(p=>Math.abs(p.value-0.6)<0.01),'actual final value');
  assert.deepEqual(writeOps(h).filter(op=>op==='automation.writeEnd'),['automation.writeEnd'],'exactly one End');
  // Keyboard: held arrow repeats are one gesture, End at key-up.
  const before=writeOps(h).length;control.keyDown('ArrowUp');
  for(let i=0;i<12;i++){control.keyDown('ArrowUp',true);control.input(0.6+i/100);control.change(0.6+i/100);await sleep(8);}
  control.keyUp(0.71);
  await until(()=>h.controller.current?.phase==='committed'&&h.controller.current.gestureID!==committed.gestureID,'keyboard commit');
  const keyboard=writeOps(h).slice(before);assert.equal(keyboard.filter(op=>op==='automation.writeBegin').length,1);assert.equal(keyboard.filter(op=>op==='automation.writeEnd').length,1);
  // Read mode: no live Begin and no static fallback edit.
  h.bindings.arm(h.trackID,volume,false);const sentBefore=h.sent.length,revision=h.mirror.snapshot.revision;
  assert.equal(control.input(0.2),false);control.change(0.2);await sleep(100);
  assert.equal(h.sent.slice(sentBefore).filter(f=>JSON.parse(f).kind==='request').length,0);assert.equal(h.mirror.snapshot.revision,revision);
  // Each gesture is one original-author Undo.
  for(let i=0;i<2;i++){const undo=await h.client.command({type:'undo'});assert.equal(undo.status,'accepted');}
  await until(()=>same(lane(h),original),'Undo restored the original lane');
 }finally{h.stop();}
});

test('a conflicting target edit yields a verified owner draft; review never reapplies it',{skip},async()=>{
 const h=await start();try{
  const original=structuredClone(lane(h));h.bindings.arm(h.trackID,volume,true);const control=new TouchControlBinding(h.bindings,h.trackID,volume);
  control.pointerDown();for(let i=1;i<=10;i++){control.input(i/20);await sleep(8);}
  await until(()=>h.controller.current?.phase==='writing'&&h.controller.current.lastAppliedSequence>=3,'applied values');
  const gain=await h.client.command({type:'setTrackGain',trackID:h.trackID,gain:0.5},undefined,{[h.trackID]:h.mirror.snapshot.trackRevisions[h.trackID]});
  assert.equal(gain.status,'accepted');
  control.pointerUp(0.5);
  await until(()=>['review','cancelled'].includes(h.controller.current?.phase),'truthful conflict outcome');
  const outcome=h.controller.current;assert.equal(outcome.phase,'review');assert.ok(outcome.draftID,'review draft retained');
  assert.ok(same(lane(h),original),'nothing authored');
  const revision=h.mirror.snapshot.revision;
  const summary=await h.controller.draft(outcome);
  const worker=new WriteDraftWorker(draftWorker);
  try{const result=await worker.download(summary,outcome.sourceRevision,command=>h.client.command(command));assert.ok(result.overview.appliedValues>=3);assert.ok(result.buffer.byteLength>0);const plot=await worker.plot(result.overview.startFrame,result.overview.endFrame+1,64);assert.ok(plot);}finally{worker.close();}
  await sleep(100);assert.equal(h.mirror.snapshot.revision,revision,'review/export leaves the project unchanged');
 }finally{h.stop();}
});

test('connection loss is uncertain, never replays End, and the same runtime retrieves its draft',{skip},async()=>{
 const h=await start();try{
  const original=structuredClone(lane(h));h.bindings.arm(h.trackID,volume,true);const control=new TouchControlBinding(h.bindings,h.trackID,volume);
  control.pointerDown();for(let i=1;i<=8;i++){control.input(i/16);await sleep(8);}
  await until(()=>h.controller.current?.lastAppliedSequence>=3,'applied values');
  const gesture=h.controller.current.gestureID,runtime=h.client.clientID;
  h.sockets.at(-1).close();
  await until(()=>h.controller.current?.phase==='uncertain','uncertain after loss');
  const ends=writeOps(h).filter(op=>op==='automation.writeEnd').length;
  control.pointerUp(0.9);
  await until(()=>h.client.state==='connected','automatic reconnect',20000);
  assert.equal(h.client.clientID,runtime,'same runtime');
  await sleep(300);
  assert.equal(writeOps(h).filter(op=>op==='automation.writeEnd').length,ends,'End is never replayed');
  assert.ok(same(lane(h),original),'disconnect never authors');
  const outcome=h.controller.outcomes.find(o=>o.gestureID===gesture);assert.ok(outcome);
  let summary;
  for(let i=0;i<100&&!summary;i++){try{summary=await h.controller.draft(outcome);}catch{await sleep(50);}}
  assert.ok(summary,'draft by known gesture for the same runtime');
  assert.equal(summary.gestureID.toUpperCase(),gesture.toUpperCase());
 }finally{h.stop();}
});
