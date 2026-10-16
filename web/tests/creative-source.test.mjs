import test from 'node:test';
import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
if(!globalThis.crypto)Object.defineProperty(globalThis,'crypto',{value:webcrypto,configurable:true});
import { WorkspaceSource } from '../dist/workspace-source.js';
import { TrackGestureRevision } from '../dist/gesture-revision.js';
import { CreativeWorkspaces } from '../dist/workspaces.js';
const a='00000000-0000-0000-0000-000000000001',b='00000000-0000-0000-0000-000000000002',clipID='00000000-0000-0000-0000-000000000003';
function setup(kind='designer'){
 let version=5,session=5,epoch=1,selected=a;const tracks=new Map([[a,{id:a,clips:[{id:clipID,start:48000}]}],[b,{id:b,clips:[]}]]),sent=[];
 const access={track:id=>tracks.get(id),version:()=>version,sessionRevision:()=>session,epoch:()=>epoch};
 const fake={generation:1,draftVersion:0,enabled:true,busy:false,current:kind,source:new WorkspaceSource(a,clipID,48000,access),sourceAccess:access,editingClipID:clipID,pattern:{bpm:120,notes:[{id:'note'}]},drums:{steps:[{step:0,velocity:.8}]},track:()=>tracks.get(selected),cursor:()=>99999,status:{},toolbar:{querySelector:()=>({}),querySelectorAll:()=>[]},dialog:{close(){fake.closed=true;}},render(){},send:async(command,id,expected,global)=>{sent.push({command,id,expected,global});version=session+1;session=version;return{revision:version};}};
 return{fake,access,tracks,sent,version:v=>version=v,session:v=>session=v,epoch:v=>epoch=v,selection:v=>selected=v};
}
test('creative drafts capture the original clip, track, frame and target version for both tools',async()=>{
 for(const kind of ['designer','beats']){const s=setup(kind);s.selection(b);await CreativeWorkspaces.prototype.commit.call(s.fake);assert.equal(s.sent.length,1);assert.equal(s.sent[0].id,a);assert.equal(s.sent[0].expected,5);assert.equal(s.sent[0].command.intent.context.trackID,a);assert.equal(s.sent[0].command.intent.context.cursorFrame,48000);assert.equal(s.sent[0].command.intent.strings.clipID,clipID);assert.equal(s.fake.closed,true);}
});
test('a changed or removed creative destination rejects locally and preserves private patterns',async()=>{
 for(const mutate of [s=>s.version(6),s=>s.tracks.delete(a),s=>s.tracks.get(a).clips=[],s=>s.epoch(2)]){const s=setup(),pattern=s.fake.pattern;mutate(s);await CreativeWorkspaces.prototype.commit.call(s.fake);assert.equal(s.sent.length,0);assert.equal(s.fake.pattern,pattern);assert.match(s.fake.status.textContent,/draft is retained/);}
});
test('foreign metadata after accepted creative insertion is never adopted by the private draft',async()=>{
 const s=setup();s.fake.send=async command=>{s.sent.push(command);s.version(7);return{revision:6};};await CreativeWorkspaces.prototype.commit.call(s.fake);assert.equal(s.fake.source.trackRevision,5);assert.equal(s.fake.closed,undefined);assert.match(s.fake.status.textContent,/retained for review/);
});
test('unconfirmed insertion and newer local edits keep the private editor open',async()=>{
 const s=setup();s.fake.send=async()=>undefined;await CreativeWorkspaces.prototype.commit.call(s.fake);assert.equal(s.fake.closed,undefined);assert.equal(s.fake.source.trackRevision,5);assert.match(s.fake.status.textContent,/not confirmed/);
 const newer=setup();const send=newer.fake.send;newer.fake.send=async(...args)=>{newer.fake.draftVersion++;return send(...args);};await CreativeWorkspaces.prototype.commit.call(newer.fake);assert.equal(newer.fake.source.trackRevision,6);assert.equal(newer.fake.closed,undefined);assert.match(newer.fake.status.textContent,/newer private draft/);
});
test('late creative replies cannot close a newly reopened workspace or reset its busy state',async()=>{
 const s=setup();let release;s.fake.send=async()=>{await new Promise(r=>release=r);s.version(6);return{revision:6};};const pending=CreativeWorkspaces.prototype.commit.call(s.fake);s.fake.generation++;s.fake.busy=true;release();await pending;assert.equal(s.fake.closed,undefined);assert.equal(s.fake.busy,true);
});
test('flushing preview cannot rebind a stale authored destination',async()=>{
 const s=setup();s.fake.previewWriter={flush:async()=>{s.version(6);return true;}};await CreativeWorkspaces.prototype.commit.call(s.fake);assert.equal(s.sent.length,0);assert.match(s.fake.status.textContent,/changed while preparing/);
});
test('explicit new-track insertion detaches the old clip without replaying against a new selected track',async()=>{
 const s=setup();s.selection(b);s.version(8);s.session(8);const pattern=s.fake.pattern;s.fake.commit=CreativeWorkspaces.prototype.commit;await CreativeWorkspaces.prototype.insertNewTrack.call(s.fake);assert.equal(s.sent.length,1);assert.equal(s.sent[0].id,undefined);assert.equal(s.sent[0].expected,undefined);assert.equal(s.sent[0].global,8);assert.equal(s.sent[0].command.intent.strings.trackID,'');assert.equal(s.sent[0].command.intent.strings.clipID,undefined);assert.equal(s.fake.pattern,pattern);
});
test('empty drum patterns never claim to have inserted a clip',async()=>{
 const s=setup('beats');s.fake.drums.steps=[];await CreativeWorkspaces.prototype.commit.call(s.fake);assert.equal(s.sent.length,0);assert.match(s.fake.status.textContent,/Add drum hits/);
});
test('connection epochs fence matching revision numbers after a server reconnect',()=>{
 let epoch=1;const guard=new TrackGestureRevision(a,()=>a,()=>5,()=>epoch);assert.equal(guard.ready(),true);epoch=2;assert.equal(guard.ready(),false);assert.equal(guard.accepted({revision:5}),false);
 const s=setup();s.epoch(2);assert.equal(s.fake.source.ready(),false);assert.equal(s.fake.source.accepted({revision:5}),false);
});

test('opening existing generated clips preserves their tempo; new patterns use arrangement tempo',()=>{
 const previous=globalThis.document;Object.defineProperty(globalThis,'document',{configurable:true,value:{activeElement:null}});
 try{for(const kind of ['designer','beats'])for(const existing of [true,false]){const s=setup(kind),source={id:clipID,start:48000,soundDesign:{bpm:83.5,notes:[]},drumPattern:{name:'Kit',bpm:83.5,steps:[]}};Object.assign(s.fake,{clip:()=>existing?source:undefined,timing:()=>({tempo:137}),presets:[{sound:{bpm:120,notes:[]}}],kits:[{id:'kit'}],drumPresets:[{pattern:{bpm:120,steps:[]}}],activeRecorded:new Map(),selected:new Set(),dialog:{open:false,dataset:{},addEventListener(){},showModal(){this.open=true;}}});CreativeWorkspaces.prototype.open.call(s.fake,kind);assert.equal((kind==='designer'?s.fake.pattern:s.fake.drums).bpm,existing?83.5:137);assert.equal(s.fake.source.frame,existing?48000:99999);}}
 finally{if(previous===undefined)delete globalThis.document;else Object.defineProperty(globalThis,'document',{configurable:true,value:previous});}
});

test('late MIDI cleanup cannot stop or close a newly reopened creative workspace',async()=>{
 let release;const fake={generation:1,releaseNotes(){},previewWriter:{cancel(){}},midiWriter:{cancel(){}},pendingMIDI:new Promise(r=>release=r),midiConnectGeneration:0,previewing:true,midiSourceID:2,intent(){assert.fail('old close must not send cleanup for a reopened workspace');},dialog:{close(){assert.fail('old close must not close reopened editor');}}};const pending=CreativeWorkspaces.prototype.close.call(fake);fake.generation++;fake.midiSourceID=7;release();await pending;assert.equal(fake.midiSourceID,7);assert.equal(fake.previewing,true);
});
test('explicit creative reload keeps its original destination rather than the background selection',async()=>{
 const original=globalThis.confirm;Object.defineProperty(globalThis,'confirm',{configurable:true,value:()=>true});try{const s=setup();s.selection(b);s.tracks.get(a).clips[0].soundDesign={notes:[]};s.fake.close=async()=>s.fake.generation++;s.fake.open=(kind,pinned)=>{s.reopened={kind,pinned};};await CreativeWorkspaces.prototype.reloadSource.call(s.fake);assert.equal(s.reopened.kind,'designer');assert.equal(s.reopened.pinned.trackID,a);assert.equal(s.reopened.pinned.clipID,clipID);
 const late=setup();late.tracks.get(a).clips[0].soundDesign={notes:[]};late.fake.close=async()=>late.fake.generation+=2;late.fake.open=()=>assert.fail('late reload must not replace new editor');await CreativeWorkspaces.prototype.reloadSource.call(late.fake);
 }finally{if(original===undefined)delete globalThis.confirm;else Object.defineProperty(globalThis,'confirm',{configurable:true,value:original});}
});
