import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { webcrypto } from 'node:crypto';
if(!globalThis.crypto)Object.defineProperty(globalThis,'crypto',{value:webcrypto,configurable:true});
import { EffectLibrary, parseEffectPresets, encodeEffectPresets, presetFromEffect, applyEffectPreset, parseEffectChains, encodeEffectChains, prepareEffectChain, checkedEffect, presetName } from '../dist/effect-presets.js';
import { EffectWorkspace } from '../dist/effect-workspace.js';
const a='00000000-0000-0000-0000-000000000001',b='00000000-0000-0000-0000-000000000002';
const metadata=[{id:'gain',name:'Gain',parameters:[{id:'gain',name:'Gain',minimum:-24,maximum:24,defaultValue:0,unit:'dB',logarithmic:false,integer:false}]}];
const preset={id:a,name:'Soft lift',kind:'gain',parameters:{gain:3}};
const point={frame:10,value:.4,curve:'smooth',curveStart:.2,curveEnd:.8};
const insert={id:b,kind:'gain',amount:.75,enabled:false,parameters:{},automation:[point],parameterAutomation:{gain:[point]},selectedAutomationParameter:'gain',pitchEdits:[{id:a,startFrame:0,endFrame:100,points:[{id:b,frame:100,note:69,curve:'linear'}]}],pitchEditsOnly:true,bakedRanges:[{startFrame:0,endFrame:100}]};
function storage(initial){let value=initial;return {getItem(){return value??null;},setItem(k,v){value=v;},value:()=>value};}

test('native effect documents roundtrip modern and legacy signal paths exactly',()=>{
    const legacy=presetFromEffect('Old gain',insert,metadata),modern=presetFromEffect('New gain',{...insert,parameters:{gain:6}},metadata);
    assert.equal(legacy.legacyAmount,.75);assert.deepEqual(legacy.parameters,{});assert.equal(modern.legacyAmount,undefined);
    const text=encodeEffectPresets([legacy,modern],metadata),value=JSON.parse(text);
    assert.equal(value.version,1);assert.equal(value.presets[0].legacyAmount,.75);assert.deepEqual(value.presets[1].parameters,{gain:6});assert.deepEqual(parseEffectPresets(text,metadata),[legacy,modern]);
});
test('parameter preset application preserves current media guards, curves, pitch and identity',()=>{
    const current=structuredClone(insert),changed=applyEffectPreset(current,preset,metadata);
    assert.equal(changed.id,current.id);assert.equal(changed.enabled,false);assert.deepEqual(changed.parameters,{gain:3});assert.deepEqual(changed.bakedRanges,current.bakedRanges);assert.deepEqual(changed.pitchEdits,current.pitchEdits);assert.deepEqual(changed.automation,current.automation);assert.deepEqual(changed.parameterAutomation,current.parameterAutomation);
    changed.automation[0].value=.9;assert.equal(current.automation[0].value,.4);
    const legacy=applyEffectPreset(changed,{...preset,parameters:{gain:12},legacyAmount:.2},metadata);assert.deepEqual(legacy.parameters,{});assert.equal(legacy.amount,.2);assert.equal(legacy.selectedAutomationParameter,null);
});
test('preset files reject future versions, extra fields, wrong IDs, aliases and unknown effects',()=>{
    const doc={version:1,presets:[preset]};
    for(const edit of [{...doc,version:2},{...doc,extra:1},{...doc,presets:[]},{...doc,presets:[{...preset,id:'bad'}]},{...doc,presets:[preset,{...preset,id:a.toLowerCase(),name:'Another'}]},{...doc,presets:[{...preset,kind:'alien'}]},{...doc,presets:[preset,{...preset,id:b,name:'Sóft lift'}]}])assert.throws(()=>parseEffectPresets(JSON.stringify(edit),metadata));
    assert.throws(()=>parseEffectPresets(' '.repeat(1048577),metadata),/1 MiB/);
});
test('physical preset values reject out-of-range and unknown fields without clamping the sound',()=>{
    for(const parameters of [{gain:25},{gain:-25},{frequency:100},{gain:'3'}])assert.throws(()=>parseEffectPresets(JSON.stringify({version:1,presets:[{...preset,parameters}]}),metadata));
    assert.throws(()=>parseEffectPresets(JSON.stringify({version:1,presets:[{...preset,legacyAmount:1.01}]}),metadata));
    const discrete=[{...metadata[0],parameters:[{...metadata[0].parameters[0],integer:true}]}];assert.throws(()=>parseEffectPresets(JSON.stringify({version:1,presets:[{...preset,parameters:{gain:1.5}}]}),discrete));
});
test('native v2 chains retain original advanced fields in files but do not bypass new media',()=>{
    const chain={id:a,name:'Vocal movement',effects:[insert],includesAutomation:true},text=encodeEffectChains([chain],metadata);
    assert.equal(JSON.parse(text).version,2);const [loaded]=parseEffectChains(text,metadata);assert.deepEqual(loaded.effects[0].bakedRanges,insert.bakedRanges);
    const [prepared]=prepareEffectChain(loaded,metadata);assert.notEqual(prepared.id,insert.id);assert.notEqual(prepared.pitchEdits[0].id,insert.pitchEdits[0].id);assert.notEqual(prepared.pitchEdits[0].points[0].id,insert.pitchEdits[0].points[0].id);assert.deepEqual(prepared.bakedRanges,[]);assert.deepEqual(prepared.automation,insert.automation);assert.equal(prepared.pitchEditsOnly,true);assert.deepEqual(insert.bakedRanges,[{startFrame:0,endFrame:100}]);
    const [plain]=prepareEffectChain({...loaded,includesAutomation:false},metadata);assert.deepEqual(plain.automation,[]);assert.deepEqual(plain.parameterAutomation,{});assert.deepEqual(plain.pitchEdits,[]);assert.equal(plain.pitchEditsOnly,false);assert.deepEqual(plain.parameters,{});assert.equal(plain.amount,.75);
});
test('chain grammar rejects oversized, duplicate, invalid and unsupported nested processor data',()=>{
    const chain={id:a,name:'Chain',effects:[insert],includesAutomation:true};
    for(const c of [{...chain,effects:[insert,insert]},{...chain,effects:[{...insert,parameters:{wrong:1}}]},{...chain,includesAutomation:'yes'},{...chain,effects:Array.from({length:65},()=>insert)},{...chain,effects:[{...insert,automation:[point,point]}]},{...chain,effects:[{...insert,pitchEdits:[{startFrame:0,endFrame:100,points:[{frame:100,note:96}]}]}]}])assert.throws(()=>parseEffectChains(JSON.stringify({version:2,chains:[c]}),metadata));
    assert.throws(()=>parseEffectChains(JSON.stringify({version:3,chains:[chain]}),metadata));assert.throws(()=>checkedEffect({...insert,parameterAutomation:{alien:[point]}},metadata));
});
test('personal library mutations are atomic when browser storage refuses the write',()=>{
    let fail=false;const local=storage(),library=new EffectLibrary({getItem:local.getItem,setItem(k,v){if(fail)throw Error('Quota');local.setItem(k,v);}},()=>metadata);
    library.savePreset(preset);const before=local.value();fail=true;assert.throws(()=>library.savePreset({...preset,id:b,name:'More'}),/Quota/);assert.equal(local.value(),before);assert.deepEqual(library.presets(),[preset]);fail=false;library.remove('preset',a);assert.equal(library.presets().length,0);library.undo();assert.deepEqual(library.presets(),[preset]);
});
test('imported libraries renew IDs and resolve colliding names without modifying the source',()=>{
    const local=storage(),library=new EffectLibrary(local,()=>metadata);library.savePreset(preset);const text=encodeEffectPresets([preset],metadata);assert.deepEqual(library.import(text),{kind:'preset',count:1});const imported=library.presets()[1];assert.notEqual(imported.id,a);assert.equal(imported.name,'Soft lift (2)');library.rename('preset',imported.id,'Clean gain');assert.equal(library.presets()[1].name,'Clean gain');assert.deepEqual(JSON.parse(text).presets,[preset]);
    const restored=new EffectLibrary(local,()=>metadata);assert.equal(restored.presets().length,2);assert.throws(()=>restored.savePreset({...preset,id:b}));assert.throws(()=>restored.savePreset(preset,'missing'));
});
test('malformed saved storage is preserved and never silently overwritten',()=>{
    for(const original of ['{broken',JSON.stringify({version:1,presets:[{id:'invalid'}],chains:[]}),JSON.stringify({version:2,presets:[],chains:[]})]){const local=storage(original),library=new EffectLibrary(local,()=>metadata);assert.ok(library.error);assert.throws(()=>library.savePreset(preset));assert.equal(local.value(),original);}
});
test('expanded workspace sends a legacy preset unchanged instead of its UI editing projection',async()=>{
    let sent;const fake={ready:()=>true,writer:{flush:async()=>true},current:()=>insert,metadata:metadata[0],effects:()=>metadata,commit:async effect=>{sent=effect;return true;},render(){this.rendered=true;}};
    const legacy={...preset,parameters:{},legacyAmount:.2};assert.equal(await EffectWorkspace.prototype.applySaved.call(fake,legacy),true);assert.deepEqual(sent.parameters,{});assert.equal(sent.amount,.2);assert.deepEqual(sent.bakedRanges,insert.bakedRanges);assert.ok(Math.abs(fake.draft.parameters.gain+14.4)<1e-10);assert.equal(fake.rendered,true);
    fake.writer.flush=async()=>false;sent=undefined;assert.equal(await EffectWorkspace.prototype.applySaved.call(fake,legacy),false);assert.equal(sent,undefined);
});

test('duplicate JSON fields and excessive nesting reject before storage or sound changes',()=>{
    assert.throws(()=>parseEffectPresets('{"version":1,"version":1,"presets":[]}',metadata),/Duplicate/);
    const text=JSON.stringify({version:1,presets:[preset]}).replace('"gain":3','"gain":3,"g\\u0061in":4');assert.throws(()=>parseEffectPresets(text,metadata),/Duplicate/);
    assert.throws(()=>parseEffectChains('['.repeat(65)+'0'+']'.repeat(65),metadata),/deeply/);
});

test('checked-in native-format fixtures preserve legacy amount and v2 rendered fields',async()=>{
    const presets=parseEffectPresets(await readFile(new URL('./fixtures/native-effect-presets.json',import.meta.url),'utf8'),metadata);assert.equal(presets[0].legacyAmount,.75);assert.deepEqual(presets[1].parameters,{gain:6});
    const chains=parseEffectChains(await readFile(new URL('./fixtures/native-effect-chains.json',import.meta.url),'utf8'),metadata);assert.deepEqual(chains[0].effects[0].bakedRanges,[{startFrame:0,endFrame:48000}]);assert.equal(chains[0].effects[0].automation[0].curveStart,.2);assert.equal(JSON.parse(encodeEffectChains(chains,metadata)).version,2);
});

test('native names count graphemes and collision suffixes never split emoji or combining sequences',()=>{
    for(const unit of ['👍🏽','🇺🇸','e\u0301']){const name=unit.repeat(80);assert.equal(presetName(name),name);assert.throws(()=>presetName(unit.repeat(81)));const local=storage(),library=new EffectLibrary(local,()=>metadata),p={...preset,name};library.savePreset(p);library.import(encodeEffectPresets([p],metadata));assert.equal(library.presets()[1].name,unit.repeat(68)+' (2)');}
    assert.throws(()=>presetName('👨‍👩‍👧‍👦'),/one line/); // Native Foundation currently classifies the ZWJ as a control.
});
test('native POSIX folding matches sharp-s, ligatures and sigma while preserving non-Roman marks',()=>{
    for(const [one,two] of [['Straße','Strasse'],['ẞ','ss'],['ﬁ','fi'],['ﬃ','ffi'],['Σ','ς'],['SÓFT','soft'],['ᾲ','α']]){const library=new EffectLibrary(storage(),()=>metadata);library.savePreset({...preset,name:one});assert.throws(()=>library.savePreset({...preset,id:b,name:two}),/unique/);}
    for(const [one,two] of [['कला','कल'],['קִ','ק'],['☀︎','☀️'],['ı','i'],['œ','oe'],['Ø','o']]){const library=new EffectLibrary(storage(),()=>metadata);library.savePreset({...preset,name:one});library.savePreset({...preset,id:b,name:two});assert.equal(library.presets().length,2);}
});
