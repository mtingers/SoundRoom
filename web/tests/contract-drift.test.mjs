import test from 'node:test';import assert from 'node:assert/strict';import {readFile} from 'node:fs/promises';import {pathToFileURL} from 'node:url';
// A browser checkpoint can consume a canonical contract committed on main before its source branch merges.
const read=path=>readFile(new URL(path,import.meta.url),'utf8');
test('browser canonical generated artifacts and grammar helpers remain byte-identical after import suffix normalization',async()=>{
 for(const file of ['generated/models.ts','generated/contract.ts','generated/write-ranges.ts','validate.ts','write_validate.ts']){
  let expected=await readFile(process.env.SOUNDROOM_PROTOCOL_SOURCE?new URL(file,pathToFileURL(process.env.SOUNDROOM_PROTOCOL_SOURCE.replace(/\/?$/,'/'))):new URL('../../protocol/'+file,import.meta.url),'utf8');
  if(!file.startsWith('generated/'))for(const module of ['generated/contract','generated/models','generated/write-ranges','write_validate'])expected=expected.replaceAll(`'./${module}'`,`'./${module}.js'`);
  assert.equal(await read('../src/'+file),expected,file);
 }
});
