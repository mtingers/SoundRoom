import {writeRanges} from './generated/write-ranges';
export const writePayloadTypes: Record<string,string> = {
 'automation.writeBegin':'RemoteAutomationWriteBeginRequest','automation.writeValue':'RemoteAutomationWriteValueRequest',
 'automation.writeEnd':'RemoteAutomationWriteEndRequest','automation.writeDraft':'RemoteAutomationWriteDraftRequest'
};
export function validateWriteModel(o:Record<string,unknown>, type:string):void {
 const bad=()=>{throw Error('Invalid '+type+' domain');};
 const number=(key:string)=>o[key] as number;
 const ranges=(writeRanges as Record<string,Record<string,readonly number[]>>)[type];
 for(const [field,[min,max]] of Object.entries(ranges??{})) if(o[field]!=null && (number(field)<min! || number(field)>max!)) bad();
 if(type==='RemoteAutomationWriteTarget') {
  const effect=o.effectID!=null, parameter=o.parameterID!=null;
  if(['volume','pan'].includes(o.kind as string) ? effect||parameter : o.kind==='effectAmount' ? !effect||parameter : !effect||!parameter) bad();
  if(parameter && (new TextEncoder().encode(o.parameterID as string).length<1 || new TextEncoder().encode(o.parameterID as string).length>128 || /[\u0000-\u001f\u007f]/u.test(o.parameterID as string))) bad();
 }
 if(type==='RemoteAutomationWriteDraftRequest' && ((o.draftID!=null)===(o.gestureID!=null))) bad();
 if(type==='RemoteAutomationWriteStamp' && (number('controlFrame')!==number('outputFrame')+number('latencyOffsetFrames') || !Number.isSafeInteger(number('outputFrame')+number('latencyOffsetFrames')))) bad();
 if(type==='RemoteAutomationWriteMapping' && number('minimum')>=number('maximum')) bad();
 if(type==='RemoteAutomationWriteLimits' && number('maximumWrittenPoints')>number('maximumLanePoints')) bad();
 if(type==='RemoteAutomationWriteValueReply' && (o.disposition==='applied' ? number('appliedSequence')!==number('sequence') : number('appliedSequence')<=number('sequence'))) bad();
 if(['RemoteAutomationWriteLeaseUpdate','RemoteAutomationWriteDraftReply','RemoteAutomationWriteDraftContent'].includes(type) && new TextEncoder().encode(o.reason as string).length>512) bad();
 if(type==='RemoteAutomationWriteLeaseUpdate' && o.phase==='committed' && o.revision==null) bad();
 if(type==='RemoteAutomationWriteDraftReply') {
  const artifact=o.artifact as Record<string,unknown>;
  if(o.firstStamp==null || o.lastStamp==null || (artifact.bytes as number)<1 || (artifact.bytes as number)>33554432 || typeof artifact.sha256!=='string' || !/^[0-9a-fA-F]{64}$/.test(artifact.sha256)) bad();
 }
 if(type==='RemoteAutomationWriteCapturedRamp' && ((o.restoring===true)===(o.sequence!=null))) bad();
 if(type==='RemoteAutomationWriteCapturedRelease') {
  if((o.lastControlFrame!=null)!==(o.lastValue!=null) || (o.lastControlFrame!=null && (o.stamp as Record<string,unknown>).controlFrame!==(o.lastControlFrame as number)+1)) bad();
 }
 if(type==='RemoteAutomationWriteDraftContent') {
  const original=o.originalPoints as Record<string,unknown>[], ramps=o.capturedRamps as Record<string,unknown>[];
  if(original.length>100000 || original.some(p=>(p.frame as number)<0 || (p.frame as number)>4147200000 || (p.value as number)<0 || (p.value as number)>1) || ramps.length<1 || ramps.length>65536 || ramps[0]?.restoring!==false) bad();
  const first=ramps[0]!.stamp as Record<string,unknown>;let previousFrame=-1, previousSequence=0;
  for(const [index,ramp] of ramps.entries()) {
   const stamp=ramp.stamp as Record<string,unknown>,frame=stamp.controlFrame as number;
   if(frame<=previousFrame || stamp.transportEpoch!==first.transportEpoch || stamp.latencyOffsetFrames!==first.latencyOffsetFrames || (ramp.restoring===true && index!==ramps.length-1)) bad();
   if(ramp.sequence!=null) {if((ramp.sequence as number)<=previousSequence) bad(); previousSequence=ramp.sequence as number;} previousFrame=frame;
  }
  if(o.terminal!=null) {const terminal=o.terminal as Record<string,unknown>,stamp=terminal.stamp as Record<string,unknown>;if(stamp.transportEpoch!==first.transportEpoch || stamp.latencyOffsetFrames!==first.latencyOffsetFrames || (terminal.lastControlFrame!=null && (terminal.lastControlFrame as number)<previousFrame)) bad();}
 }
}
export function validateWriteIntent(intent:Record<string,unknown>):string|undefined {
 const type=writePayloadTypes[intent.operation as string]; if(!type) return;
 const context=intent.context as Record<string,unknown>;
 if(Object.entries(context).some(([key,value])=>key==='clipIDs'? !Array.isArray(value)||value.length!==0 : key==='cursorFrame'?value!==0:value!=null)) throw Error('Write has no client cursor/context');
 for(const field of ['integers','numbers','strings','flags']) if(Object.keys(intent[field] as object).length!==0) throw Error('Write requires only its typed payload');
 if(typeof intent.payload!=='string' || atob(intent.payload).length>4096) throw Error('Missing/oversized Write payload');
 return type;
}
