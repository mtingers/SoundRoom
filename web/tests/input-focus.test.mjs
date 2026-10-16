import test from 'node:test';
import assert from 'node:assert/strict';
import { ownsShortcutInput, acceptWorkspaceShortcut, bindInstrumentKey, navigateTimelineItem, openWorkspaceDialog } from '../dist/input-focus.js';
import { CreativeWorkspaces } from '../dist/workspaces.js';
import { AutomationEditor } from '../dist/automation.js';
import { PitchWorkspace } from '../dist/pitch-workspace.js';
function event(extra={}){return {target:{closest:()=>null},defaultPrevented:false,isComposing:false,code:'Space',key:' ',repeat:false,preventDefault(){this.prevented=true;},stopPropagation(){this.stopped=true;},...extra};}
test('native controls, content editing, IME and consumed events keep their keyboard ownership',()=>{
    for(const selector of ['button','input','select','summary','a[href]','[contenteditable]','[role="slider"]']){const target={closest:s=>{assert.ok(s.includes(selector==='[contenteditable]'?'[contenteditable]':selector));return {};}};assert.equal(ownsShortcutInput(target),true);assert.equal(acceptWorkspaceShortcut(event({target})),false);}
    assert.equal(acceptWorkspaceShortcut(event({isComposing:true})),false);assert.equal(acceptWorkspaceShortcut(event({defaultPrevented:true})),false);assert.equal(acceptWorkspaceShortcut(event()),true);
    const repeat=event({repeat:true});assert.equal(acceptWorkspaceShortcut(repeat),false);assert.equal(repeat.prevented,true);
});
test('Space on an action button never also toggles Designer or Pitch audition',()=>{
    const e=event({target:{closest:()=>({})}}),fake={enabled:true,togglePreview(){assert.fail('button Space must have one native action');}};
    CreativeWorkspaces.prototype.keyDown.call(fake,e);PitchWorkspace.prototype.key.call({canOperate(){assert.fail('must not inspect audition on a focused button');}},e);assert.equal(e.prevented,undefined);
});
test('held virtual buttons press once and release on key-up or focus loss',()=>{
    const handlers={},button={disabled:false,addEventListener(k,fn){handlers[k]=fn;}};let on=0,off=0;bindInstrumentKey(button,()=>on++,()=>off++);
    handlers.keydown(event());handlers.keydown(event({repeat:true}));assert.equal(on,1);handlers.keyup(event());assert.equal(off,1);handlers.blur();assert.equal(off,1);
    handlers.keydown(event({code:'Enter'}));handlers.blur();assert.equal(on,2);assert.equal(off,2);handlers.keyup(event({code:'Enter'}));assert.equal(off,2);
    button.disabled=true;handlers.keydown(event());assert.equal(on,2);
});
test('typed note release remains independent of the new focused control',()=>{
    const sent=[],fake={keys:new Map([['KeyA',60]]),liveNote:async(...args)=>sent.push(args)};
    CreativeWorkspaces.prototype.keyUp.call(fake,event({code:'KeyA',target:{closest:()=>({})}}));assert.deepEqual(sent,[[60,false]]);assert.equal(fake.keys.size,0);
});
function fakeDialog(){const handlers={};return {open:false,dataset:{},handlers,showModal(){this.open=true;this.shown=(this.shown??0)+1;},addEventListener(name,fn){handlers[name]=fn;},focus(){this.focused=true;}};}
test('modal focus returns to a usable opener and falls back locally after a mixer rerender',()=>{
    const prior=globalThis.document,focus=[],fallback={focus:o=>focus.push(['timeline',o])};
    const opener={isConnected:true,closest:()=>null,focus:o=>focus.push(['opener',o])};
    Object.defineProperty(globalThis,'document',{configurable:true,value:{activeElement:opener,querySelectorAll:()=>[],querySelector:()=>fallback}});
    try{const d=fakeDialog(),canvas={isConnected:true,focus:o=>focus.push(['canvas',o])};openWorkspaceDialog(d,canvas);assert.equal(d.shown,1);openWorkspaceDialog(d,canvas);assert.equal(d.shown,1);d.open=false;d.handlers.close();assert.deepEqual(focus.map(x=>x[0]),['canvas','opener']);
        opener.isConnected=false;const other=fakeDialog();openWorkspaceDialog(other);other.open=false;other.handlers.close();assert.equal(focus.at(-1)[0],'timeline');
    }finally{if(prior===undefined)delete globalThis.document;else Object.defineProperty(globalThis,'document',{configurable:true,value:prior});}
});
test('closed-parent and disabled openers are skipped; delayed close cannot steal reopened focus',()=>{
    const prior=globalThis.document;let restored=0;const fallback={focus:()=>restored++},opener={isConnected:true,disabled:true,closest:()=>null,focus:()=>assert.fail('disabled opener')};
    Object.defineProperty(globalThis,'document',{configurable:true,value:{activeElement:opener,querySelectorAll:()=>[],querySelector:()=>fallback}});
    try{const d=fakeDialog();openWorkspaceDialog(d);d.handlers.close();assert.equal(restored,0);d.open=false;d.handlers.close();assert.equal(restored,1);opener.disabled=false;opener.closest=()=>({});const closedParent=fakeDialog();openWorkspaceDialog(closedParent);closedParent.open=false;closedParent.handlers.close();assert.equal(restored,2);}
    finally{if(prior===undefined)delete globalThis.document;else Object.defineProperty(globalThis,'document',{configurable:true,value:prior});}
});

test('temporal item navigation selects first/last and stable same-time neighbours without editing',()=>{
 const a={t:5},b={t:0},c={t:5},items=[a,b,c],pos=p=>p.t;assert.equal(navigateTimelineItem(items,undefined,'Home',pos),b);assert.equal(navigateTimelineItem(items,undefined,'End',pos),c);assert.equal(navigateTimelineItem(items,b,']',pos),a);assert.equal(navigateTimelineItem(items,a,']',pos),c);assert.equal(navigateTimelineItem(items,c,'[',pos),a);assert.equal(navigateTimelineItem(items,b,'[',pos),b);assert.equal(navigateTimelineItem([],a,'Home',pos),undefined);assert.equal(navigateTimelineItem(items,a,'ArrowLeft',pos),undefined);
});
test('Pitch chart can select and reveal targets with a keyboard without authoring audio changes',()=>{
 const first={id:'first',frame:100,note:60},last={id:'last',frame:10000,note:64},fake={regions:[{points:[last,first]}],selected:undefined,point(){return this.regions[0].points.find(p=>p.id===this.selected);},updatePoint(){this.pointPosition.value=String(this.point().frame);},pointPosition:{value:''},x:frame=>frame,y:()=>60,width:800,height:400,scope:[0,48000],scale:.1,semitonePixels:10,message(v){this.announcement=v;},commit(){assert.fail('navigation must not author');}};
 PitchWorkspace.prototype.key.call(fake,event({key:'Home',code:'Home'}));assert.equal(fake.selected,'first');PitchWorkspace.prototype.key.call(fake,event({key:']',code:'BracketRight'}));assert.equal(fake.selected,'last');assert.ok(fake.offset>0);assert.match(fake.announcement,/Selected E4/);assert.equal(fake.regions[0].points[0],last);
});
test('Designer note selection and select-all remain private draft navigation',()=>{
 const notes=[{id:'later',startBeat:5,durationBeats:1,pitch:60,velocity:.5},{id:'first',startBeat:0,durationBeats:1,pitch:64,velocity:.8}],fake={enabled:true,current:'designer',pattern:{notes},selected:new Set(),status:{},canvas:{clientWidth:300,clientHeight:250},beatPixels:90,offset:0,topPitch:80,revealNote:CreativeWorkspaces.prototype.revealNote,snapshot(){assert.fail('selection must not create undo history');},send(){assert.fail('selection must not reach server');}};
 CreativeWorkspaces.prototype.keyDown.call(fake,event({key:'Home',code:'Home'}));assert.deepEqual([...fake.selected],['first']);CreativeWorkspaces.prototype.keyDown.call(fake,event({key:']',code:'BracketRight'}));assert.deepEqual([...fake.selected],['later']);assert.ok(fake.offset>0);CreativeWorkspaces.prototype.keyDown.call(fake,event({key:'a',code:'KeyA',ctrlKey:true}));assert.equal(fake.selected.size,2);assert.match(fake.status.textContent,/2 notes/);assert.equal(fake.pattern.notes,notes);
});

test('automation keyboard selection reveals a private point without writing or losing conflict context',()=>{
 const points=[{frame:0,value:.2},{frame:100000,value:.8}],view={offset:0,pixelsPerSecond:100,follow:true},reports=[];let redraws=0;const fake={points:()=>points,selectedFrame:undefined,target:{name:'Volume'},positionInput:{},valueInput:{},updateFields(p){this.positionInput.value=String(p.frame);this.valueInput.value=String(p.value);},x:AutomationEditor.prototype.x,sampleRate:()=>10000,view:()=>view,width:300,navigate:()=>redraws++,report:m=>reports.push(m),blocked:true,writer(){assert.fail('navigation must not author a curve');}};
 AutomationEditor.prototype.key.call(fake,event({key:'End',code:'End'}));assert.equal(fake.selectedFrame,100000);assert.ok(view.offset>0);assert.equal(view.follow,false);assert.equal(redraws,1);assert.match(reports[0],/Private draft/);assert.equal(fake.points(),points);
});
