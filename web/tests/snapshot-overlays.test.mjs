import test from 'node:test';
import assert from 'node:assert/strict';
import { SnapshotOverlays } from '../dist/snapshot-overlays.js';
import { SessionMirror } from '../dist/model.js';
const id='11111111-1111-4111-8111-111111111111';
const snapshot=()=>({serverID:id,serverName:'Host',revision:4,trackRevisions:{},session:{version:9,name:'Song',timing:{tempo:120,beatsPerBar:4,beatUnit:4},tracks:[],masterTrack:{id,clips:[]}},transport:{frame:0,isPlaying:false,isRecording:false,isPreparing:false,hostTime:0},devices:[],inputDeviceID:0,outputDeviceID:0,peers:[],assets:[],status:'Old'});
const device={id:5,uid:'mic',name:'Mic',inputChannels:1,outputChannels:0};
const peer={id,name:'Editor',role:'editor'};
test('initial presence and devices before snapshotBegin survive immutable pages',()=>{
 const overlays=new SnapshotOverlays(),mirror=new SessionMirror();
 overlays.retainPresence([peer]);overlays.retainDevices([device],5,6);overlays.retainStatus('Live');
 assert.equal(mirror.snapshot,undefined);overlays.install(mirror,snapshot(),10);
 assert.deepEqual(mirror.snapshot.peers,[peer]);assert.deepEqual(mirror.snapshot.devices,[device]);assert.equal(mirror.snapshot.inputDeviceID,5);assert.equal(mirror.snapshot.outputDeviceID,6);assert.equal(mirror.snapshot.status,'Live');
});
test('only the latest overlay survives pages and transport retains its receipt clock',()=>{
 const overlays=new SnapshotOverlays(),mirror=new SessionMirror();mirror.applySnapshot(snapshot(),0);
 overlays.retainPresence([peer]);overlays.retainPresence([]);overlays.retainDevices([device],5,6);overlays.retainDevices([],7,8);
 overlays.retainTransport({...snapshot().transport,frame:100,isPlaying:true},5);
 overlays.retainTransport({...snapshot().transport,frame:200,isPlaying:false},8);
 overlays.install(mirror,snapshot(),100);
 assert.deepEqual(mirror.snapshot.peers,[]);assert.deepEqual(mirror.snapshot.devices,[]);assert.equal(mirror.snapshot.inputDeviceID,7);assert.equal(mirror.snapshot.transport.frame,200);assert.equal(mirror.snapshot.transport.isPlaying,false);
});
test('installed overlays are consumed and reset prevents reconnect/server leakage',()=>{
 const overlays=new SnapshotOverlays(),mirror=new SessionMirror();
 overlays.retainPresence([peer]);overlays.install(mirror,snapshot(),0);overlays.install(mirror,snapshot(),1);assert.deepEqual(mirror.snapshot.peers,[]);
 overlays.retainPresence([peer]);overlays.retainDevices([device],5,6);overlays.retainStatus('Old connection');overlays.retainTransport({...snapshot().transport,frame:300},2);overlays.reset();overlays.install(mirror,snapshot(),3);
 assert.deepEqual(mirror.snapshot.peers,[]);assert.deepEqual(mirror.snapshot.devices,[]);assert.equal(mirror.snapshot.status,'Old');assert.equal(mirror.snapshot.transport.frame,0);
});
test('overlays have bounded latest values rather than unbounded event queues',()=>{
 const overlays=new SnapshotOverlays(),mirror=new SessionMirror();
 for(let i=0;i<10000;i++)overlays.retainPresence([{...peer,name:String(i)}]);
 assert.throws(()=>overlays.retainPresence(Array(257).fill(peer)));assert.throws(()=>overlays.retainDevices(Array(257).fill(device),0,0));assert.throws(()=>overlays.retainStatus('x'.repeat(1024*1024)));
 overlays.install(mirror,snapshot(),0);assert.equal(mirror.snapshot.peers[0].name,'9999');
});

test('stale delta status cannot replace a live status regardless of arrival order',()=>{
 for(const staleLast of [false,true]){
  const overlays=new SnapshotOverlays(),mirror=new SessionMirror();
  if(staleLast){overlays.retainStatus('Live');overlays.retainDeltaStatus('Stale R4',4);}else{overlays.retainDeltaStatus('Stale R4',4);overlays.retainStatus('Live');}
  const current=snapshot();current.revision=5;overlays.install(mirror,current,1);assert.equal(mirror.snapshot.status,'Live');
 }
});
test('status from an applied newer delta follows live-event arrival order',()=>{
 for(const liveLast of [false,true]){
  const overlays=new SnapshotOverlays(),mirror=new SessionMirror();const current=snapshot();current.revision=5;mirror.applySnapshot(current,0);
  if(liveLast){overlays.retainDeltaStatus('Delta R6',6);overlays.retainStatus('Live later');}else{overlays.retainStatus('Live earlier');overlays.retainDeltaStatus('Delta R6',6);}
  mirror.applyDelta({revision:6,tracks:[],controls:[],removedTrackIDs:[],trackRevisions:{},assets:[],status:'Delta R6'});
  overlays.retainDeltaStatus('Stale R4 last',4);overlays.apply(mirror,5);assert.equal(mirror.snapshot.status,liveLast?'Live later':'Delta R6');
 }
});
