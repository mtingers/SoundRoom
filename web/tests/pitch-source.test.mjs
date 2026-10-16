import test from 'node:test';
import assert from 'node:assert/strict';
import {webcrypto} from 'node:crypto';
if(!globalThis.crypto)Object.defineProperty(globalThis,'crypto',{value:webcrypto,configurable:true});
import {PitchWorkspace} from '../dist/pitch-workspace.js';
const id='00000000-0000-0000-0000-000000000001',effectID='00000000-0000-0000-0000-000000000002';
const encoded=value=>Buffer.from(JSON.stringify(value)).toString('base64');
const region=()=>({startFrame:0,endFrame:48000,points:[{id:'00000000-0000-0000-0000-000000000003',frame:0,note:60}]});
function setup(){
 let epoch=1;const sent=[],effect={id:effectID,kind:'pitchCorrection',amount:.5,parameters:{reference:440},pitchEdits:[region()],pitchEditsOnly:true},track={id,realtimeEffects:[effect],clips:[{id:'clip'}]};
 const fake=Object.assign(Object.create(PitchWorkspace.prototype),{mirror:{snapshot:{revision:5,trackRevisions:{[id]:5},session:{tracks:[track]}}},trackID:id,effectID,scope:[0,48000],regions:[region()],parameters:{reference:440},lastTrackRevision:5,sourceEpoch:1,epoch:()=>epoch,generation:1,writing:0,busy:false,blocked:false,draft:false,enabled:()=>true,canOperate:()=>true,message(text){fake.status=text;},renderToolbar(){},renderControls(){},updatePoint(){},fit(){},meta:()=>({parameters:[{id:'reference',defaultValue:440}]}),send:async(command,trackID,expected)=>{sent.push({command,trackID,expected});if(command.type==='updateTrackControls'){track.realtimeEffects=structuredClone(command.controls.realtimeEffects);fake.mirror.snapshot.trackRevisions[id]=6;return{revision:6};}return{revision:5,payload:encoded({points:[],regions:[region()],isOutput:false,targetsAreDraft:true})};}});
 return{fake,sent,track,epoch:v=>epoch=v,version:v=>fake.mirror.snapshot.trackRevisions[id]=v};
}
test('Pitch preserves revision and private targets after a rejected analysis',async()=>{
 const s=setup(),regions=s.fake.regions; s.fake.send=async()=>{s.version(7);return undefined;};await s.fake.analyze('pitch.analyze');assert.equal(s.fake.lastTrackRevision,5);assert.equal(s.fake.regions,regions);assert.equal(s.fake.blocked,true);assert.equal(await s.fake.commit(regions,'Private'),false);
});
test('Pitch binds requests to its captured target version and accepts its own exact metadata',async()=>{
 const s=setup();assert.equal(await s.fake.commit([region()],'Edit'),true);assert.equal(s.sent[0].trackID,id);assert.equal(s.sent[0].expected,5);assert.equal(s.fake.lastTrackRevision,6);assert.equal(s.fake.draft,false);assert.equal(s.fake.writing,0);
});
test('Pitch accepted-with-newer-foreign metadata remains a private conflicted draft',async()=>{
 const s=setup(),draft=[region()];s.fake.send=async()=>{s.version(7);return{revision:6};};assert.equal(await s.fake.commit(draft,'Edit'),false);assert.equal(s.fake.lastTrackRevision,5);assert.equal(s.fake.regions,draft);assert.equal(s.fake.draft,true);assert.equal(s.fake.blocked,true);
});
test('Pitch does not erase newer local edits while its earlier change is accepted',async()=>{
 const s=setup(),next=[{...region(),points:[{...region().points[0],note:62}]}];s.fake.send=async(command)=>{s.track.realtimeEffects=command.controls.realtimeEffects;s.fake.regions=next;s.fake.parameters.reference=442;s.version(6);return{revision:6};};assert.equal(await s.fake.commit([region()],'Earlier'),true);assert.equal(s.fake.regions,next);assert.equal(s.fake.parameters.reference,442);assert.equal(s.fake.draft,true);assert.equal(s.fake.lastTrackRevision,6);
});
test('Pitch rejects deleted or reconnected sources without sending or adopting equal revision numbers',async()=>{
 for(const mutate of [s=>s.epoch(2),s=>s.fake.mirror.snapshot.session.tracks=[]]){const s=setup();mutate(s);assert.equal(await s.fake.commit(s.fake.regions,'Edit'),false);assert.equal(s.sent.length,0);assert.equal(s.fake.lastTrackRevision,5);}
});
test('late Pitch commit cannot mutate a reopened editor or its writing count',async()=>{
 const s=setup();let finish;s.fake.send=()=>new Promise(resolve=>finish=resolve);const pending=s.fake.commit(s.fake.regions,'Edit');s.fake.generation++;s.fake.writing=0;s.fake.lastTrackRevision=20;s.fake.draft=true;finish({revision:6});assert.equal(await pending,false);assert.equal(s.fake.lastTrackRevision,20);assert.equal(s.fake.writing,0);assert.equal(s.fake.draft,true);
});
test('Pitch analysis discards neither private targets nor its binding when foreign metadata wins the reply race',async()=>{
 const s=setup(),regions=s.fake.regions;s.fake.send=async()=>{s.version(7);return{revision:5,payload:encoded({points:[],regions:[],isOutput:false,targetsAreDraft:true})};};await s.fake.analyze('pitch.analyze');assert.equal(s.fake.regions,regions);assert.equal(s.fake.lastTrackRevision,5);assert.equal(s.fake.blocked,true);
});
test('read-only Pitch analysis can confirm unchanged target despite an unrelated later session version',async()=>{
 const s=setup();s.fake.send=async()=>({revision:8,payload:encoded({points:[],regions:[region()],isOutput:false,targetsAreDraft:true})});await s.fake.analyze('pitch.analyze');assert.equal(s.fake.lastTrackRevision,5);assert.equal(s.fake.blocked,false);assert.equal(s.fake.busy,false);
});
test('Pitch explicit reload requires consent to discard an unconfirmed private target',async()=>{
 const s=setup(),regions=s.fake.regions;s.fake.draft=true;globalThis.confirm=()=>false;await s.fake.analyze('pitch.analyze');assert.equal(s.sent.length,0);assert.equal(s.fake.regions,regions);delete globalThis.confirm;
});
test('a queued Pitch gesture cannot send against a reloaded source revision',async()=>{
 const s=setup(),writer=s.fake.writer('Drag');s.fake.generation++;s.fake.lastTrackRevision=6;s.version(6);writer.offer(s.fake.regions);await writer.flush();assert.equal(s.sent.length,0);
});
test('Pitch apply-to-audio refuses stale targets and late results cannot close a reopened editor',async()=>{
 const s=setup();s.version(6);await s.fake.bake();assert.equal(s.sent.length,0);s.version(5);s.fake.blocked=false;s.fake.draft=false;globalThis.confirm=()=>true;let finish;s.fake.send=()=>new Promise(resolve=>finish=resolve);s.fake.close=()=>assert.fail('late bake cannot close');const pending=s.fake.bake();s.fake.generation++;s.fake.busy=false;finish({revision:6});await pending;assert.equal(s.fake.busy,false);delete globalThis.confirm;
});
test('Pitch close and audition cleanup cannot close or reset a reopened editor',async()=>{
 const s=setup();let finish;s.fake.send=()=>new Promise(resolve=>finish=resolve);s.fake.dialog={close(){assert.fail('late close cannot affect reopened dialog');}};s.fake.auditioning=true;const pending=s.fake.close();s.fake.generation++;s.fake.auditioning=true;finish({revision:5});await pending;assert.equal(s.fake.auditioning,true);
});
