import test from 'node:test';
import assert from 'node:assert/strict';
import { EffectWorkspace } from '../dist/effect-workspace.js';
import { sameEffectSettings } from '../dist/effect-models.js';
import { sameAutomationPoints } from '../dist/automation.js';
const effect={id:'fx',kind:'gain',amount:.5,enabled:true,parameters:{gain:1},automation:[],parameterAutomation:{},pitchEdits:[],pitchEditsOnly:false,bakedRanges:[]};
function setup(){const track={id:'track',name:'Voice',gain:1,pan:0,realtimeEffects:[structuredClone(effect)]},sent=[],fake={trackID:'track',trackRevision:5,writing:0,generation:0,blocked:false,ready:EffectWorkspace.prototype.ready,conflict:EffectWorkspace.prototype.conflict,track:()=>track,enabled:()=>true,mirror:{snapshot:{trackRevisions:{track:5}}},current:()=>track.realtimeEffects[0],status:{},send:async(command,id,expected)=>{sent.push({command,id,expected});return{revision:5};}};return{fake,track,sent};}
test('expanded effects bind explicit source revisions and accept only their own installed change',async()=>{
 const {fake,track,sent}=setup(),changed={...effect,parameters:{gain:6}};fake.send=async(command,id,expected)=>{sent.push({command,id,expected});track.realtimeEffects=command.controls.realtimeEffects;fake.mirror.snapshot.trackRevisions.track=6;return{revision:6};};assert.equal(await EffectWorkspace.prototype.commit.call(fake,changed,'Gain','gesture'),true);assert.equal(sent[0].expected,5);assert.equal(fake.trackRevision,6);assert.equal(fake.writing,0);
});
test('newer foreign metadata after acceptance never advances the old effect draft revision',async()=>{
 const {fake,track,sent}=setup();fake.send=async(command,id,expected)=>{sent.push({command,id,expected});track.realtimeEffects=[{...effect,parameters:{gain:9}}];fake.mirror.snapshot.trackRevisions.track=7;return{revision:6};};assert.equal(await EffectWorkspace.prototype.commit.call(fake,{...effect,parameters:{gain:6}},'Gain','gesture'),false);assert.equal(fake.trackRevision,5);assert.equal(track.realtimeEffects[0].parameters.gain,9);assert.match(fake.status.textContent,/reopen/);assert.equal(await EffectWorkspace.prototype.commit.call(fake,effect,'Gain','next'),false);assert.equal(sent.length,1);
});
test('an early receipt without target metadata and rejected edits fail closed',async()=>{
 for(const reply of [{revision:6},undefined]){const {fake}=setup();fake.send=async()=>reply;assert.equal(await EffectWorkspace.prototype.commit.call(fake,{...effect,parameters:{gain:6}},'Gain','gesture'),false);assert.equal(fake.trackRevision,5);assert.equal(fake.writing,0);}
});
test('throwing sends release the in-flight counter; verified no-ops retain their old target version',async()=>{
 const {fake}=setup();fake.send=async()=>{throw Error('Disconnected');};assert.equal(await EffectWorkspace.prototype.commit.call(fake,effect,'Gain','gesture'),false);assert.equal(fake.writing,0);
 fake.blocked=false;fake.send=async()=>({revision:10});assert.equal(await EffectWorkspace.prototype.commit.call(fake,effect,'No-op','gesture'),true);assert.equal(fake.trackRevision,5);
});
test('no-op comparisons use native Float32 scalars, exact timeline frames and order-independent parameters',()=>{
 const point={frame:100000001,value:.1,curveStart:.2,curveEnd:.8},native={...point,value:Math.fround(.1),curveStart:Math.fround(.2),curveEnd:Math.fround(.8),curve:'linear'};
 assert.equal(sameAutomationPoints([point],[native]),true);assert.equal(sameAutomationPoints([point],[{...native,frame:100000000}]),false);assert.equal(sameAutomationPoints([point],[{...native,value:.1001}]),false);
 const one={...effect,parameters:{gain:.1,mix:.2},automation:[point]},two={...effect,parameters:{mix:Math.fround(.2),gain:Math.fround(.1)},automation:[native],selectedAutomationParameter:null};assert.equal(sameEffectSettings(one,two),true);assert.equal(sameEffectSettings(one,{...two,parameters:{gain:.1001,mix:.2}}),false);assert.equal(sameEffectSettings(one,{...two,automation:[{...native,frame:100000000}]}),false);
});

test('expanded FX rejects an equal-numbered source from a different connection epoch',async()=>{
 const {fake,sent}=setup();fake.sourceEpoch=1;fake.epoch=()=>2;assert.equal(await EffectWorkspace.prototype.commit.call(fake,effect,'Edit','gesture'),false);assert.equal(sent.length,0);assert.equal(fake.blocked,true);assert.equal(fake.trackRevision,5);
});
test('late expanded FX receipt cannot bless or change the counter of a reopened editor',async()=>{
 const {fake}=setup();let finish;fake.send=()=>new Promise(resolve=>finish=resolve);const pending=EffectWorkspace.prototype.commit.call(fake,effect,'Edit','gesture');fake.generation++;fake.trackRevision=20;fake.writing=0;fake.status.textContent='New editor';finish({revision:6});assert.equal(await pending,false);assert.equal(fake.trackRevision,20);assert.equal(fake.writing,0);assert.equal(fake.status.textContent,'New editor');
});
test('uncertain FX edits remain blocked even if unchanged target metadata later arrives',async()=>{
 const {fake,sent}=setup();fake.send=async(command,id,expected)=>{sent.push({command,id,expected});return undefined;};assert.equal(await EffectWorkspace.prototype.commit.call(fake,effect,'Edit','gesture'),false);fake.send=()=>assert.fail('must explicitly reopen to review');assert.equal(await EffectWorkspace.prototype.commit.call(fake,effect,'Retry','gesture'),false);assert.equal(sent.length,1);
});
test('FX bake checks captured version and cannot close a reopened workspace',async()=>{
 const {fake,track,sent}=setup();track.clips=[{id:'clip'}];fake.effectID='fx';fake.mirror.snapshot.trackRevisions.track=6;await EffectWorkspace.prototype.bake.call(fake);assert.equal(sent.length,0);fake.blocked=false;fake.mirror.snapshot.trackRevisions.track=5;globalThis.confirm=()=>true;let finish;fake.send=(command,id,expected)=>{sent.push({command,id,expected});return new Promise(resolve=>finish=resolve);};fake.close=()=>assert.fail('late bake must not close');const pending=EffectWorkspace.prototype.bake.call(fake);assert.equal(sent[0].expected,5);fake.generation++;fake.writing=0;finish({revision:6});await pending;assert.equal(fake.writing,0);delete globalThis.confirm;
});
test('pending FX preset flush cannot apply into a different reopened source',async()=>{
 const {fake}=setup();let finish;fake.writer={flush:()=>new Promise(resolve=>finish=resolve)};fake.effects=()=>[];fake.commit=()=>assert.fail('wrong source');const pending=EffectWorkspace.prototype.applySaved.call(fake,{name:'Preset'});fake.generation++;finish(true);assert.equal(await pending,false);
});
test('FX navigation keeps the original track/effect and ignores late cleanup from a newer editor',async()=>{
 for(const method of ['openAutomation','openPitchChart']){const {fake}=setup();fake.effectID='fx';fake.current=()=>({...effect,parameters:{gain:1}});fake.automate=()=>assert.fail('must not navigate after reopening');fake.openPitch=()=>assert.fail('must not navigate after reopening');let finish;fake.close=()=>{fake.generation++;return new Promise(resolve=>finish=resolve);};const pending=EffectWorkspace.prototype[method].call(fake,'gain');fake.generation++;finish();await pending;}
});
