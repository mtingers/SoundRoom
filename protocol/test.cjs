// Run after compiling validate.ts to a temporary directory with tsc.
let checks = 0;
const assert = new Proxy(require('node:assert/strict'), {
 get(target,key) { const value=Reflect.get(target,key); return typeof value==='function' ? (...args)=>{checks++;return Reflect.apply(value,target,args);} : value; }
});
const {validateRequest,validateModel} = require(process.argv[2]);
const id = '12345678-1234-4234-8234-123456789abc';
const base = {version:3,kind:'request',id,clientID:id,expectedTrackRevisions:{[id]:2},command:{type:'seek',frame:48000}};
assert.equal(validateRequest(base),base);
for (const change of [
 {version:2},{clientID:'bad'},{role:'operator'}, {expectedRevision:2**53},
 {expectedTrackRevisions:{[id]:-1}}, {command:{type:'seek',frame:true}},
 {command:{type:'seek',frame:1.5}}, {command:{type:'seek',frame:-1}},
 {command:{type:'seek',frame:1,selector:'unsafe'}}, {command:{type:'toString'}},
 {command:{type:'uploadChunk',id,offset:0,data:'malformed!'}}
]) assert.throws(()=>validateRequest({...base,...change}));
assert.doesNotThrow(()=>validateRequest({...base,command:{type:'setDevices',inputUID:'backend:opaque'}}));
assert.doesNotThrow(()=>validateRequest({...base,command:{type:'setTrackPan',trackID:id,pan:-0.3}}));
assert.doesNotThrow(()=>validateModel({version:9,tracks:[]},'Session'));
assert.doesNotThrow(()=>validateModel({version:10},'Session'));
assert.throws(()=>validateModel({version:12},'Session'));
assert.throws(()=>validateModel({tempo:120,beatsPerBar:4,beatUnit:3},'MusicalTiming'));
assert.doesNotThrow(()=>validateModel({seed:'18446744073709551615'},'OrganicSound'));
assert.throws(()=>validateModel({seed:'18446744073709551616'},'OrganicSound'));
assert.throws(()=>validateModel({seed:17},'OrganicSound'));
assert.throws(()=>validateModel({notes:[],waveform:'externalPlugin'},'SoundDesignPattern'));
assert.throws(()=>validateModel({id,pitch:true,velocity:0.8,startBeat:0,durationBeats:1},'SoundDesignNote'));
const context = {clipIDs:[],cursorFrame:0};
const invalidPattern = Buffer.from(JSON.stringify({notes:[],waveform:'externalPlugin'})).toString('base64');
assert.throws(()=>validateRequest({...base,command:{type:'editor',intent:{operation:'designer.commit',context,integers:{},numbers:{},strings:{},flags:{},payload:invalidPattern}}}));
assert.throws(()=>validateRequest({...base,command:{type:'editor',intent:{operation:'run.shell',context,integers:{},numbers:{},strings:{},flags:{}}}}));
for (const role of ['viewer','editor','operator']) assert.doesNotThrow(()=>validateModel({id,name:'Peer',role},'RemotePeer'));
assert.throws(()=>validateModel({id,name:'Peer',role:'admin'},'RemotePeer'));
const badMaster = Buffer.from(JSON.stringify([{kind:'externalPlugin'}])).toString('base64');
assert.throws(()=>validateRequest({...base,command:{type:'editor',intent:{operation:'master.applyProposal',context,integers:{},numbers:{},strings:{},flags:{},payload:badMaster}}}));
assert.doesNotThrow(()=>validateRequest({...base,command:{type:'editor',intent:{operation:'master.proposal',context,integers:{},numbers:{targetLUFS:-14,intensity:0.5},strings:{},flags:{}}}}));
for(const patch of [{trackID:id,gain:4,muted:false},{trackID:id,gain:null,pan:-1}]) assert.doesNotThrow(()=>validateModel(patch,'RemoteTrackControlPatch'));
for(const patch of [{trackID:id},{trackID:id,gain:null},{trackID:id,gain:4.00001},{trackID:id,pan:-1.01},{trackID:id,muted:1}]) assert.throws(()=>validateModel(patch,'RemoteTrackControlPatch'));
for(const gain of [0,1.5,2,4]) assert.doesNotThrow(()=>validateModel({gain,pan:0},'Track'));
for(const gain of [-0.01,4.0001]) assert.throws(()=>validateModel({gain},'Track'));
assert.throws(()=>validateModel({gain:4,pan:1.001},'Track'));
const scalarDelta={revision:1,tracks:[],controls:[],removedTrackIDs:[],trackRevisions:{},metadataUpdated:false,assets:[]};
assert.doesNotThrow(()=>validateModel(scalarDelta,'RemoteSessionDelta'));
assert.throws(()=>validateModel({...scalarDelta,controlPatches:[{trackID:id,gain:0},{trackID:id.toUpperCase(),pan:0}]},'RemoteSessionDelta'));
const badFeatures=Buffer.from(JSON.stringify([1])).toString('base64');
assert.throws(()=>validateRequest({...base,command:{type:'editor',intent:{operation:'client.features',context,integers:{},numbers:{},strings:{},flags:{},payload:badFeatures}}}));
for(const [index,fixture] of require('./fixtures/automation-write-models.v3.json').entries()) {
 if(fixture.valid) assert.doesNotThrow(()=>validateModel(fixture.value,fixture.type),`Write fixture ${index}`);
 else assert.throws(()=>validateModel(fixture.value,fixture.type),`Write fixture ${index}`);
}
const beginPayload=Buffer.from(JSON.stringify({gestureID:id,trackID:id,target:{kind:'volume'},mode:'touch'})).toString('base64');
const writeBegin={...base,command:{type:'editor',intent:{operation:'automation.writeBegin',context,integers:{},numbers:{},strings:{},flags:{},payload:beginPayload}}};
assert.doesNotThrow(()=>validateRequest(writeBegin));
for(const change of [{expectedTrackRevisions:{}},{expectedTrackRevisions:{[id]:2,'22345678-1234-4234-8234-123456789abc':1}},{expectedRevision:2}]) assert.throws(()=>validateRequest({...writeBegin,...change}));
const valuePayload=Buffer.from(JSON.stringify({gestureID:id,sequence:1,value:0.5})).toString('base64');
const writeValue={...writeBegin,expectedTrackRevisions:{},command:{type:'editor',intent:{...writeBegin.command.intent,operation:'automation.writeValue',payload:valuePayload}}};
assert.doesNotThrow(()=>validateRequest(writeValue));
assert.throws(()=>validateRequest({...writeValue,expectedTrackRevisions:{[id]:2}}));
console.log(`${checks} portable request/model-validator checks passed`);
