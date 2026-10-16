import test from 'node:test';import assert from 'node:assert/strict';import {readFileSync} from 'node:fs';import vm from 'node:vm';import {webcrypto} from 'node:crypto';import {validateModel} from '../dist/validate.js';import {validateSnapshot} from '../dist/model.js';
import {trackControls} from '../dist/effects.js';
import {waitFor} from './wait-for.mjs';
test('explicit browser fixture conforms to canonical snapshot/capability grammar',()=>{const scope={crypto:webcrypto,Uint8Array,structuredClone,queueMicrotask,btoa:s=>Buffer.from(s).toString('base64'),addEventListener(){}};scope.window=scope;vm.runInNewContext(readFileSync(new URL('./browser/fixture.js',import.meta.url),'utf8'),scope);validateModel(scope.fixture.snapshot,'RemoteSnapshot');validateModel(scope.fixture.capabilities,'RemoteCapabilities');assert.ok(validateSnapshot(scope.fixture.snapshot));assert.equal(scope.fixture.snapshot.session.tracks.length,100);assert.equal(scope.fixture.snapshot.session.tracks.flatMap(t=>t.clips).length,10000);assert.ok(JSON.stringify(scope.fixture.snapshot).length<8*1024*1024);});
test('browser fixture emits a canonical controls delta and correlated receipt after an effect edit',async()=>{
 const scope={crypto:webcrypto,Uint8Array,structuredClone,queueMicrotask,btoa:s=>Buffer.from(s).toString('base64'),addEventListener(){}};scope.window=scope;
 vm.runInNewContext(readFileSync(new URL('./browser/fixture.js',import.meta.url),'utf8'),scope);
 const socket=new scope.WebSocket(),messages=[];socket.onmessage=e=>messages.push(JSON.parse(e.data));
 await waitFor(()=>socket.readyState===1,'fixture socket opens');
 const track=scope.fixture.snapshot.session.tracks[0],controls=trackControls(track);controls.realtimeEffects[0].parameters.gain=3;
 const requestID='00000000-0000-4000-8000-000000999999';
 socket.send(JSON.stringify({version:3,kind:'request',id:requestID,command:{type:'updateTrackControls',controls,name:'Gain'}}));
 await waitFor(()=>messages.some(m=>m.kind==='reply'&&m.requestID===requestID),'fixture edit receipt');
 const delta=messages.find(m=>m.kind==='event'&&m.event.type==='delta').event.delta;
 validateModel(delta,'RemoteSessionDelta');assert.equal(delta.metadataUpdated,false);
 assert.equal(delta.controls[0].realtimeEffects[0].parameters.gain,3);assert.equal(delta.trackRevisions[track.id],5);
 assert.equal(scope.fixture.snapshot.session.tracks[0].id,track.id);assert.equal(scope.fixture.snapshot.revision,5);
});
test('editing an existing routed track preserves its output sends and compressor key',()=>{
 const track={id:'00000000-0000-4000-8000-000000000001',name:'Voice',color:0,gain:1,pan:0,muted:false,solo:false,armed:false,realtimeEffects:[],mixAutomation:{},routing:{role:'audio',outputID:'00000000-0000-4000-8000-000000000002',sends:[{trackID:'00000000-0000-4000-8000-000000000003',level:0.25}],sidechainSourceID:'00000000-0000-4000-8000-000000000004'}};
 const controls=trackControls(track);validateModel(controls,'RemoteTrackControls');assert.deepEqual(controls.routing,track.routing);
});
