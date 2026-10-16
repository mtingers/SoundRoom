import test from 'node:test';
import assert from 'node:assert/strict';
import {PitchWorkspace} from '../dist/pitch-workspace.js';
import {MasterWorkspace} from '../dist/master-workspace.js';
import {CreativeWorkspaces} from '../dist/workspaces.js';

test('closing a preparing Pitch workspace cancels only its owned audition',async()=>{
  const sent=[],fake={generation:7,busy:true,auditioning:false,canOperate:()=>true,send:async command=>sent.push(command),dialog:{close(){fake.closed=true;}},stopAudition:PitchWorkspace.prototype.stopAudition};
  await PitchWorkspace.prototype.close.call(fake);
  assert.equal(fake.generation,8);assert.equal(fake.closed,true);
  assert.deepEqual(sent.map(c=>c.intent.operation),['audition.stop']);
  assert.deepEqual(sent[0].intent.strings,{kind:'pitch'});
  assert.equal(sent.some(c=>c.intent.operation==='jobs.cancel'||c.intent.operation==='transport.stop'),false);
});
test('closing a Master analysis ignores late results without cancelling another shared job',async()=>{
  let cancelled=false;const fake={generation:4,busy:true,writer:{cancel(){cancelled=true;}},send(){assert.fail('close must not cancel shared jobs');},dialog:{close(){fake.closed=true;}}};
  await MasterWorkspace.prototype.close.call(fake);
  assert.equal(fake.generation,5);assert.equal(cancelled,true);assert.equal(fake.closed,true);
});
test('creative preview stop names its kind so a delayed stop cannot cancel another workspace',async()=>{
  for(const [current,kind] of [['designer','designer'],['beats','drums']]){
    const calls=[],fake={canOperate:true,previewing:true,current,intent:async(...args)=>calls.push(args),status:{}};
    await CreativeWorkspaces.prototype.togglePreview.call(fake);
    assert.equal(fake.previewing,false);assert.deepEqual(calls[0],['audition.stop',undefined,{},{},{kind}]);
  }
});
