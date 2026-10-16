import test from 'node:test';
import assert from 'node:assert/strict';
import {MasterWorkspace} from '../dist/master-workspace.js';
const id='00000000-0000-0000-0000-000000000001',masterID='00000000-0000-0000-0000-000000000002',effectID='00000000-0000-0000-0000-000000000003';
const encoded=value=>Buffer.from(JSON.stringify(value)).toString('base64');
const effect=()=>({id:effectID,kind:'limiter',parameters:{ceiling:-1},enabled:true});
const report=()=>({integratedLUFS:-15,truePeakDB:-1,crestDB:12,bassFraction:.1,airFraction:.1,sections:[],analyzedFrames:48000,sourceFingerprint:'same'});
function setup(){
 let epoch=1;const sent=[],snapshot={revision:5,trackRevisions:{[id]:5,[masterID]:5},session:{tracks:[{id}],masterTrack:{id:masterID,realtimeEffects:[]},masteringAnalysis:report()}};
 const fake=Object.assign(Object.create(MasterWorkspace.prototype),{mirror:{snapshot},epoch:()=>epoch,generation:1,busy:false,target:-14,intensity:.65,manualVersion:0,proposalSerial:0,proposalReady:true,proposed:[effect()],report:snapshot.session.masteringAnalysis,enabled:()=>true,render(){},status:{},writer:{cancel(){},offer(value){fake.queued=value;}},dialog:{close(){fake.closed=true;}},done(){fake.doneCalled=true;},send:async(command,trackID,expected)=>{sent.push({command,trackID,expected});if(command.intent.operation==='master.applyProposal'){snapshot.session.masterTrack.realtimeEffects=structuredClone(fake.proposed);snapshot.trackRevisions[masterID]=6;return{revision:6};}return{revision:5,payload:encoded([effect()])};}});
 fake.bindSource();fake.proposalSourceToken=fake.sourceToken;return{fake,sent,snapshot,epoch:v=>epoch=v,trackVersion:v=>snapshot.trackRevisions[id]=v,masterVersion:v=>snapshot.trackRevisions[masterID]=v};
}
test('Master analysis source includes Master rack revision, source tracks and connection epoch',()=>{
 for(const mutate of [s=>s.masterVersion(6),s=>s.trackVersion(6),s=>s.epoch(2),s=>s.snapshot.session.tracks=[]]){const s=setup();assert.equal(s.fake.stale(),false);mutate(s);assert.equal(s.fake.stale(),true);}
});
test('queued Master proposal retains the source identity captured before another client edit',async()=>{
 const s=setup();s.fake.queueProposal();const queued=s.fake.queued,source=s.fake.sourceToken;s.trackVersion(6);assert.equal(await s.fake.proposal(queued.target,queued.intensity,queued.source,queued.generation,queued.serial,queued.manualVersion),false);assert.equal(s.sent.length,0);assert.equal(s.fake.sourceToken,source);assert.equal(s.fake.proposalReady,false);
});
test('an already stale Master cannot silently queue a proposal against newer metadata',()=>{
 const s=setup(),source=s.fake.sourceToken;s.masterVersion(8);s.fake.queueProposal();assert.equal(s.fake.queued,undefined);assert.equal(s.fake.sourceToken,source);assert.match(s.fake.status.textContent,/kept here/);
});
test('Master keeps its previous private proposal when a reply races foreign source metadata',async()=>{
 const s=setup(),proposed=s.fake.proposed,source=s.fake.sourceToken;s.fake.send=async()=>{s.masterVersion(7);return{revision:6,payload:encoded([effect()])};};assert.equal(await s.fake.proposal(-14,.65),false);assert.equal(s.fake.proposed,proposed);assert.equal(s.fake.sourceToken,source);assert.equal(s.fake.proposalReady,false);
});
test('Master preserves newer manual proposal edits instead of replacing them with in-flight generated settings',async()=>{
 const s=setup(),proposed=s.fake.proposed;s.fake.send=async()=>{proposed[0].parameters.ceiling=-2;s.fake.manualVersion++;return{revision:5,payload:encoded([effect()])};};assert.equal(await s.fake.proposal(-14,.65),false);assert.equal(s.fake.proposed,proposed);assert.equal(s.fake.proposed[0].parameters.ceiling,-2);assert.match(s.fake.status.textContent,/newer manual/);assert.equal(s.fake.proposalReady,true);
});
test('Master ignores older queued proposal settings and late results from another workspace generation',async()=>{
 for(const mutate of [s=>s.fake.proposalSerial++,s=>s.fake.generation++]){const s=setup(),proposed=s.fake.proposed;s.fake.send=async()=>{mutate(s);return{revision:5,payload:encoded([effect()])};};assert.equal(await s.fake.proposal(-14,.65),false);assert.equal(s.fake.proposed,proposed);}
});
test('Master apply uses its captured Master version and closes only after exact own metadata',async()=>{
 const s=setup();await s.fake.apply();assert.equal(s.sent[0].trackID,masterID);assert.equal(s.sent[0].expected,5);assert.equal(s.fake.closed,true);assert.equal(s.fake.doneCalled,true);assert.equal(s.fake.busy,false);
});
test('Master accepted-with-newer Master or arrangement metadata retains its private proposal',async()=>{
 for(const mutate of [s=>s.masterVersion(7),s=>{s.masterVersion(6);s.trackVersion(7);},s=>{s.masterVersion(6);s.epoch(2);}]){const s=setup(),proposed=s.fake.proposed;s.fake.send=async()=>{mutate(s);return{revision:6};};await s.fake.apply();assert.equal(s.fake.closed,undefined);assert.equal(s.fake.doneCalled,undefined);assert.equal(s.fake.proposed,proposed);assert.equal(s.fake.proposalReady,false);assert.equal(s.fake.busy,false);}
});
test('late Master apply cannot close a reopened dialog or change its busy state',async()=>{
 const s=setup();let finish;s.fake.send=()=>new Promise(resolve=>finish=resolve);const pending=s.fake.apply();s.fake.generation++;s.fake.busy=false;s.fake.proposalReady=true;finish({revision:6});await pending;assert.equal(s.fake.closed,undefined);assert.equal(s.fake.busy,false);assert.equal(s.fake.proposalReady,true);
});
test('Master uncertain apply retains proposal and cannot immediately replay the mutation',async()=>{
 const s=setup(),proposed=s.fake.proposed;s.fake.send=async()=>undefined;await s.fake.apply();assert.equal(s.fake.closed,undefined);assert.equal(s.fake.proposed,proposed);assert.equal(s.fake.proposalReady,false);assert.equal(s.fake.busy,false);await s.fake.apply();assert.equal(s.sent.length,0);
});
test('rejected Master analysis retains its source binding and proposal',async()=>{
 const s=setup(),source=s.fake.sourceToken,proposed=s.fake.proposed,initial=s.fake.report;globalThis.confirm=()=>true;s.fake.send=async()=>{s.trackVersion(9);return undefined;};await s.fake.analyze();assert.equal(s.fake.sourceToken,source);assert.equal(s.fake.proposed,proposed);assert.equal(s.fake.report,initial);assert.equal(s.fake.busy,false);delete globalThis.confirm;
});
test('Master explicit re-analysis requires consent before discarding proposal settings',async()=>{
 const s=setup();globalThis.confirm=()=>false;await s.fake.analyze();assert.equal(s.sent.length,0);assert.equal(s.fake.proposed.length,1);delete globalThis.confirm;
});
test('Master no-op apply must verify the observed rack and keep target revision despite unrelated metadata',async()=>{
 const s=setup();s.snapshot.session.masterTrack.realtimeEffects=structuredClone(s.fake.proposed);s.fake.send=async()=>({revision:8});await s.fake.apply();assert.equal(s.fake.closed,true);assert.equal(s.snapshot.trackRevisions[masterID],5);
});
test('Master request exceptions release busy state but preserve private drafts',async()=>{
 for(const op of ['apply','analyze']){const s=setup(),proposed=s.fake.proposed;globalThis.confirm=()=>true;s.fake.send=async()=>{throw Error('Disconnected');};await s.fake[op]();assert.equal(s.fake.busy,false);assert.equal(s.fake.proposed,proposed);assert.equal(s.fake.closed,undefined);assert.equal(s.fake.proposalReady,false);delete globalThis.confirm;}
});

test('Master retains changed loudness controls rather than installing an earlier in-flight proposal',async()=>{
 const s=setup(),proposed=s.fake.proposed;s.fake.send=async()=>{s.fake.intensity=.9;return{revision:5,payload:encoded([effect()])};};assert.equal(await s.fake.proposal(-14,.65),false);assert.equal(s.fake.proposed,proposed);assert.equal(s.fake.intensity,.9);assert.equal(s.fake.proposalReady,false);
});
test('a valid Master proposal remains source-bound even after newer unrelated session-only metadata',async()=>{
 const s=setup();s.fake.send=async()=>{s.snapshot.revision=12;return{revision:12,payload:encoded([effect()])};};assert.equal(await s.fake.proposal(-14,.65),true);assert.equal(s.fake.proposalReady,true);assert.equal(s.fake.stale(),false);assert.equal(s.fake.sourceMasterRevision,5);
});
