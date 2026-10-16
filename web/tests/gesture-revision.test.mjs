import test from 'node:test';
import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
if(!globalThis.crypto)Object.defineProperty(globalThis,'crypto',{value:webcrypto,configurable:true});
import { TrackGestureRevision, ownAcceptedTrackRevision } from '../dist/gesture-revision.js';
import { AutomationEditor } from '../dist/automation.js';
const point=value=>({frame:10,value,curve:'linear',curveStart:0,curveEnd:1});
function setup(){let version=5;const track={id:'track',name:'Voice',gain:1,pan:0,muted:false,solo:false,armed:false,realtimeEffects:[],mixAutomation:{volume:[point(.2)]}},sent=[],messages=[],fake={canEdit:true,editingAllowed:()=>true,blocked:false,generation:0,track:()=>track,revision:()=>version,metadata:[],canvas:{setAttribute(){}},report:m=>messages.push(m),send:async(command,id,expected)=>{sent.push({command,id,expected});return {revision:version};},conflict:AutomationEditor.prototype.conflict};return{fake,track,sent,messages,version:v=>version=v};}
const target={id:'volume',name:'Volume',parameter:'volume'};

test('gesture captures both track identity and version rather than following a new selection',()=>{
 let identity='a',version=5;const guard=new TrackGestureRevision('a',()=>identity,()=>version);assert.equal(guard.ready(),true);version=6;assert.equal(guard.ready(),false);version=5;identity='b';assert.equal(guard.ready(),false);assert.equal(guard.accepted({revision:5}),false);
});
test('only exact own metadata advances a gesture; newer unrelated session metadata is harmless',()=>{
 let version=5;const guard=new TrackGestureRevision('a',()=> 'a',()=>version);version=6;assert.equal(guard.accepted({revision:6}),true);assert.equal(guard.revision,6);assert.equal(guard.ready(),true);version=8;assert.equal(guard.accepted({revision:7}),false);assert.equal(guard.revision,6);
 assert.equal(ownAcceptedTrackRevision(5,5,{revision:10}),undefined);assert.equal(ownAcceptedTrackRevision(5,5,{revision:10},true),5);assert.equal(ownAcceptedTrackRevision(5,6,{revision:6}),6);assert.equal(ownAcceptedTrackRevision(5,7,{revision:6}),undefined);assert.equal(ownAcceptedTrackRevision(5,6,true),undefined);
});
test('a foreign edit before sending preserves the private curve and cannot send with a new version',async()=>{
 const state=setup(),writer=AutomationEditor.prototype.writer.call(state.fake,'Edit automation',target);state.version(6);writer.offer([point(.8)]);assert.equal(await writer.flush(),false);assert.equal(state.sent.length,0);assert.equal(state.fake.blocked,true);assert.deepEqual(state.fake.draft,[point(.8)]);assert.deepEqual(state.track.mixAutomation.volume,[point(.2)]);assert.match(state.messages[0],/Reload lane/);
});
test('coalesced updates advance only after their own correlated receipt and metadata fence',async()=>{
 const state=setup();let release;state.fake.send=async(command,id,expected)=>{state.sent.push({command,id,expected});if(state.sent.length===1)await new Promise(r=>release=r);state.version(state.sent.length===1?6:7);state.track.mixAutomation=command.controls.mixAutomation;return {revision:state.sent.length===1?6:7};};
 const writer=AutomationEditor.prototype.writer.call(state.fake,'Edit automation',target);writer.offer([point(.4)]);writer.offer([point(.7)]);assert.equal(state.sent[0].expected,5);release();assert.equal(await writer.flush(),true);assert.deepEqual(state.sent.map(r=>r.expected),[5,6]);assert.equal(state.sent[0].command.gestureID,state.sent[1].command.gestureID);assert.equal(state.track.mixAutomation.volume[0].value,.7);
});
test('accepted-with-newer-foreign-metadata does not bless a stale pending pointer value',async()=>{
 const state=setup();let release;state.fake.send=async(command,id,expected)=>{state.sent.push({command,id,expected});await new Promise(r=>release=r);state.track.mixAutomation.volume=[point(.9)];state.version(7);return {revision:6};};
 const writer=AutomationEditor.prototype.writer.call(state.fake,'Edit automation',target);writer.offer([point(.4)]);writer.offer([point(.8)]);release();assert.equal(await writer.flush(),false);assert.equal(state.sent.length,1);assert.deepEqual(state.track.mixAutomation.volume,[point(.9)]);assert.equal(state.fake.blocked,true);writer.offer([point(.1)]);assert.equal(state.sent.length,1);
});
test('early receipt without installed target metadata and uncertain sends never continue a gesture',async()=>{
 for(const outcome of [{revision:6},undefined]){const state=setup();state.fake.send=async(command,id,expected)=>{state.sent.push({command,id,expected});return outcome;};const writer=AutomationEditor.prototype.writer.call(state.fake,'Edit automation',target);writer.offer([point(.8)]);assert.equal(await writer.flush(),false);assert.equal(state.sent.length,1);assert.equal(state.fake.blocked,true);assert.deepEqual(state.fake.draft,[point(.8)]);writer.offer([point(.3)]);assert.equal(state.sent.length,1);}
});
test('verified no-op accepts unrelated session advancement without adopting a foreign target version',async()=>{
 const state=setup();state.fake.send=async(command,id,expected)=>{state.sent.push({command,id,expected});return {revision:10};};const writer=AutomationEditor.prototype.writer.call(state.fake,'Edit automation',target);writer.offer([point(.2)]);assert.equal(await writer.flush(),true);assert.equal(state.sent[0].expected,5);assert.equal(state.fake.blocked,false);
});
test('explicit lane reload discards only the private draft and preserves server automation',()=>{
 const state=setup();state.fake.blocked=true;state.fake.draft=[point(.8)];state.fake.canvas.removeAttribute=()=>{};AutomationEditor.prototype.reviewServerLane.call(state.fake);assert.equal(state.fake.blocked,false);assert.equal(state.fake.draft,undefined);assert.deepEqual(state.track.mixAutomation.volume,[point(.2)]);assert.equal(state.sent.length,0);
});

test('explicit lane navigation fences late accepted work and discards queued updates from the old lane',async()=>{
 const state=setup();let release;state.fake.send=async(command,id,expected)=>{state.sent.push({command,id,expected});await new Promise(r=>release=r);state.track.mixAutomation=command.controls.mixAutomation;state.version(6);return {revision:6};};state.fake.canvas.removeAttribute=()=>{};
 const writer=AutomationEditor.prototype.writer.call(state.fake,'Edit automation',target);writer.offer([point(.4)]);writer.offer([point(.8)]);AutomationEditor.prototype.reviewServerLane.call(state.fake);state.fake.draft=[point(.1)];release();assert.equal(await writer.flush(),false);assert.equal(state.sent.length,1);assert.equal(state.fake.blocked,false);assert.deepEqual(state.fake.draft,[point(.1)]);assert.deepEqual(state.track.mixAutomation.volume,[point(.4)]);
});

test('automation gesture retains its private curve across a reconnect with equal track versions',async()=>{
 const state=setup();let epoch=1;state.fake.epoch=()=>epoch;const writer=AutomationEditor.prototype.writer.call(state.fake,'Edit automation',target);epoch=2;writer.offer([point(.8)]);assert.equal(await writer.flush(),false);assert.equal(state.sent.length,0);assert.equal(state.fake.blocked,true);assert.deepEqual(state.fake.draft,[point(.8)]);
});
