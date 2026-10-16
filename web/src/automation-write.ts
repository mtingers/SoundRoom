import {randomUUID} from './crypto.js';
import type {Command,Reply,Track} from './model.js';
import type {RemoteAutomationWriteTarget as WriteTarget,RemoteAutomationWriteBeginReply as BeginReply,RemoteAutomationWriteValueReply as ValueReply,RemoteAutomationWriteLeaseUpdate as LeaseUpdate,RemoteAutomationWriteMapping as Mapping,RemoteAutomationWriteStamp as Stamp,RemoteAutomationWriteDraftReply as DraftReply} from './generated/models.js';
import {validateModel} from './validate.js';
import {encodeBase64,UncertainCommandError} from './sdk.js';
import {payload} from './transfers.js';
import type {EffectMetadata} from './effects.js';
export const writeOperations=['automation.writeBegin','automation.writeValue','automation.writeEnd','automation.writeDraft'] as const;
export function writeCommand(operation:typeof writeOperations[number],value:unknown):Command {
 const type={ 'automation.writeBegin':'RemoteAutomationWriteBeginRequest','automation.writeValue':'RemoteAutomationWriteValueRequest','automation.writeEnd':'RemoteAutomationWriteEndRequest','automation.writeDraft':'RemoteAutomationWriteDraftRequest'}[operation];
 validateModel(value,type);return {type:'editor',intent:{operation,context:{cursorFrame:0,clipIDs:[]},integers:{},numbers:{},strings:{},flags:{},payload:encodeBase64(new TextEncoder().encode(JSON.stringify(value)))}};
}
export function automationWriteAvailable(capabilities:Record<string,unknown>,role:string,connected:boolean) {
 return connected&&role==='operator'&&Array.isArray(capabilities.features)&&capabilities.features.includes('automation-write')&&Array.isArray(capabilities.commands)&&capabilities.commands.includes('editor')&&Array.isArray(capabilities.editorOperations)&&writeOperations.every(op=>(capabilities.editorOperations as unknown[]).includes(op));
}
export function sameWriteTarget(a:WriteTarget,b:WriteTarget) {return a.kind===b.kind&&a.effectID?.toUpperCase()===b.effectID?.toUpperCase()&&a.parameterID===b.parameterID;}
export function writeTargetSupported(track:Track,target:WriteTarget,metadata:EffectMetadata[]) {
 try{validateModel(target,'RemoteAutomationWriteTarget');}catch{return false;}
 if(target.kind==='volume')return track.gain<=1.5 || (track.mixAutomation?.volume?.length??0)>0;
 if(target.kind==='pan')return true;
 const effect=track.realtimeEffects.find(e=>(e as {id?:string}).id?.toUpperCase()===target.effectID?.toUpperCase()) as {kind:string;parameters?:Record<string,number>}|undefined;if(!effect)return false;
 if(target.kind==='effectAmount')return !Object.keys(effect.parameters??{}).length;
 const parameter=metadata.find(m=>m.id===effect.kind)?.parameters.find(p=>p.id===target.parameterID);
 return !!parameter&&!!Object.keys(effect.parameters??{}).length&&parameter.id!=='manualTarget'&&!parameter.integer&&!parameter.choices?.length&&parameter.maximum>parameter.minimum&&(!parameter.logarithmic||parameter.minimum>0);
}
export function writeMappingMatches(track:Track,target:WriteTarget,mapping:Mapping,metadata:EffectMetadata[]) {
 const scale=(track as Track&{volumeAutomationScale?:string}).volumeAutomationScale;let minimum=0,maximum=1,unit='%',logarithmic=false;
 if(target.kind==='volume'){maximum=1.5;unit='gain';}
 else if(target.kind==='pan'){minimum=-1;unit='';}
 else if(target.kind==='effectParameter'){const effect=track.realtimeEffects.find(e=>(e as {id?:string}).id?.toUpperCase()===target.effectID?.toUpperCase()) as {kind:string}|undefined,p=metadata.find(m=>m.id===effect?.kind)?.parameters.find(p=>p.id===target.parameterID);if(!p)return false;({minimum,maximum,unit,logarithmic}=p);}
 return !mapping.integer&&Math.fround(mapping.minimum)===Math.fround(minimum)&&Math.fround(mapping.maximum)===Math.fround(maximum)&&mapping.unit===unit&&mapping.logarithmic===logarithmic&&(target.kind==='volume'?!scale||mapping.interpolationScale===scale:mapping.interpolationScale==='linear');
}
export type WritePhase='idle'|'beginning'|'writing'|'finalizing'|'committed'|'cancelled'|'review'|'uncertain';
export interface WriteOutcome {gestureID:string;trackID:string;target:WriteTarget;sourceRevision:number;serverID?:string;credentialID?:string;runtimeID?:string;phase:WritePhase;reason:string;draftID?:string;revision?:number;requestID?:string;lastAppliedSequence:number;stamp?:Stamp;}
export interface WriteEnvironment {
 available():boolean;epoch():number;serverID?():string|undefined;credentialID?():string|undefined;runtimeID?():string|undefined;version(trackID:string):number|undefined;track(trackID:string):Track|undefined;
 metadata():EffectMetadata[];playing():boolean;looping():boolean;
 send(command:Command,versions?:Record<string,number>):Promise<Reply>;
 changed?():void;
}
interface Lease extends WriteOutcome {epoch:number;trackRevision:number;sequence:number;initial?:BeginReply;pending?:number;activeValue:boolean;heldValue?:number;hostFinalizing?:boolean;ending?:boolean;cancel?:boolean;terminal?:LeaseUpdate;}
/** One ordinary value in flight and one latest pending value. End has one bounded final-value slot. No browser clock creates points. */
export class AutomationWriteController {
 private lease?:Lease;private renew?:ReturnType<typeof setInterval>;private generation=0;
 private results:readonly WriteOutcome[]=[];
 constructor(private environment:WriteEnvironment){}
 private snapshot(lease:WriteOutcome):WriteOutcome {const {gestureID,trackID,target,sourceRevision,serverID,credentialID,runtimeID,phase,reason,draftID,revision,requestID,lastAppliedSequence,stamp}=lease;return Object.freeze({gestureID,trackID,target:Object.freeze({...target}),sourceRevision,serverID,credentialID,runtimeID,phase,reason,draftID,revision,requestID,lastAppliedSequence,stamp:stamp?Object.freeze({...stamp}):undefined});}
 get current():Readonly<WriteOutcome>|undefined{return this.lease?this.snapshot(this.lease):undefined;}
 get mapping():Mapping|undefined{return this.lease?.initial?{...this.lease.initial.mapping}:undefined;}
 get outcomes():readonly WriteOutcome[]{return this.results;}
 supported(trackID:string,target:WriteTarget){const track=this.environment.track(trackID);return this.environment.available()&&!!track&&writeTargetSupported(track,target,this.environment.metadata());}
 get busy(){return !!this.lease&&['beginning','writing','finalizing'].includes(this.lease.phase);}
 private changed(){this.environment.changed?.();}
 private held(lease:Lease,generation:number){return this.lease===lease&&this.generation===generation&&lease.epoch===this.environment.epoch();}
 private stopRenew(){clearInterval(this.renew);this.renew=undefined;}
 private remember(lease:Lease){this.results=Object.freeze([this.snapshot(lease),...this.results.filter(r=>r.gestureID!==lease.gestureID)].slice(0,16));}
 private fail(lease:Lease,reason:string,error?:unknown,release=false){this.stopRenew();lease.phase=error instanceof UncertainCommandError?'uncertain':'review';lease.reason=reason;if(error instanceof UncertainCommandError)lease.requestID=error.requestID;lease.pending=undefined;lease.ending=true;this.remember(lease);this.changed();if(release&&lease.epoch===this.environment.epoch()&&this.environment.available()){const generation=this.generation;void this.environment.send(writeCommand('automation.writeEnd',{gestureID:lease.gestureID,finalSequence:lease.sequence,cancel:true})).then(reply=>{if(this.held(lease,generation)){const update=payload<LeaseUpdate>(reply.payload);validateModel(update,'RemoteAutomationWriteLeaseUpdate');this.update(update);}}).catch(()=>{});}}
 async begin(trackID:string,target:WriteTarget):Promise<boolean> {
  if(this.busy||!this.environment.available()||!this.environment.playing()||this.environment.looping())return false;
  const track=this.environment.track(trackID),trackRevision=this.environment.version(trackID);if(!track||trackRevision===undefined||!writeTargetSupported(track,target,this.environment.metadata()))return false;
  this.stopRenew();const generation=++this.generation,lease:Lease={gestureID:randomUUID().toUpperCase(),trackID,target:{...target},sourceRevision:trackRevision,serverID:this.environment.serverID?.(),credentialID:this.environment.credentialID?.(),runtimeID:this.environment.runtimeID?.(),phase:'beginning',reason:'Waiting for the server…',lastAppliedSequence:0,epoch:this.environment.epoch(),trackRevision,sequence:0,activeValue:false};this.lease=lease;this.changed();
  try{
   const reply=await this.environment.send(writeCommand('automation.writeBegin',{gestureID:lease.gestureID,trackID,target,mode:'touch'}),{[trackID]:trackRevision});
   if(!this.held(lease,generation))return false;
   const result=payload<BeginReply>(reply.payload);validateModel(result,'RemoteAutomationWriteBeginReply');
   if(result.gestureID.toUpperCase()!==lease.gestureID.toUpperCase()||result.trackID.toUpperCase()!==trackID.toUpperCase()||!sameWriteTarget(result.target,target)||result.trackRevision!==trackRevision||this.environment.version(trackID)!==trackRevision||!writeMappingMatches(track,target,result.mapping,this.environment.metadata()))throw Error('The live automation source could not be confirmed.');
   lease.initial=result;lease.stamp=result.stamp;lease.heldValue??=result.initialValue;if(lease.terminal)return false;if(lease.hostFinalizing){this.changed();return true;}lease.phase='writing';lease.reason='Touch is recording on the server.';
   this.renew=setInterval(()=>{if(this.held(lease,generation)&&lease.phase==='writing'&&!lease.ending)this.offer(lease.heldValue??lease.initial!.initialValue);},Math.max(50,Math.min(1000,Math.floor(result.limits.leaseTimeoutMilliseconds/3))));
   this.changed();if(lease.ending)await this.finish(lease,generation);else this.drain(lease,generation);return true;
  }catch(error){if(this.held(lease,generation))this.fail(lease,error instanceof Error?error.message:'Live automation could not start.',error,true);return false;}
 }
 offer(value:number){const lease=this.lease;if(!lease||!['beginning','writing'].includes(lease.phase)||lease.ending||!Number.isFinite(value)||value<0||value>1)return false;lease.pending=value;lease.heldValue=value;if(lease.initial)this.drain(lease,this.generation);return true;}
 private drain(lease:Lease,generation:number){if(lease.activeValue||lease.pending===undefined||lease.ending||!this.held(lease,generation))return;const value=lease.pending;lease.pending=undefined;lease.activeValue=true;void this.value(lease,generation,value).finally(()=>{if(this.held(lease,generation)){lease.activeValue=false;if(lease.phase==='writing'&&!lease.ending)this.drain(lease,generation);}});}
 private async value(lease:Lease,generation:number,value:number){const sequence=++lease.sequence;try{
  const reply=await this.environment.send(writeCommand('automation.writeValue',{gestureID:lease.gestureID,sequence,value}));if(!this.held(lease,generation)||lease.terminal)return;
  const result=payload<ValueReply>(reply.payload);validateModel(result,'RemoteAutomationWriteValueReply');
  if(result.gestureID.toUpperCase()!==lease.gestureID.toUpperCase()||result.sequence!==sequence||result.appliedSequence>lease.sequence||result.stamp.transportEpoch!==lease.initial?.stamp.transportEpoch||result.appliedSequence>=lease.lastAppliedSequence&&result.stamp.playbackClock<(lease.stamp?.playbackClock??0)||result.disposition==='applied'&&Math.fround(result.targetValue)!==Math.fround(value))throw Error('The server automation acknowledgement did not match this gesture.');
  if(result.appliedSequence>=lease.lastAppliedSequence){lease.lastAppliedSequence=result.appliedSequence;lease.stamp=result.stamp;lease.initial!.initialValue=result.targetValue;this.changed();}
 }catch(error){if(this.held(lease,generation)&&!lease.terminal)this.fail(lease,error instanceof Error?error.message:'A live automation value could not be confirmed.',error,!lease.ending);}}
 async end(cancel=false){const lease=this.lease;if(!lease||!['beginning','writing'].includes(lease.phase)||lease.ending)return;lease.ending=true;lease.cancel=cancel;this.stopRenew();if(lease.initial)await this.finish(lease,this.generation);else this.changed();return this.current;}
 private async finish(lease:Lease,generation:number){
  this.stopRenew();
  // The last unsent pointer value gets a single bounded slot beside the existing value; End never queues behind an acknowledgement.
  if(!lease.cancel&&lease.pending!==undefined){const value=lease.pending;lease.pending=undefined;void this.value(lease,generation,value);}
  else lease.pending=undefined;
  lease.phase='finalizing';lease.reason=lease.cancel?'Returning to Read…':'Finishing the recorded gesture…';this.changed();
  try{const reply=await this.environment.send(writeCommand('automation.writeEnd',{gestureID:lease.gestureID,finalSequence:lease.sequence,cancel:lease.cancel??false}));if(!this.held(lease,generation))return;const update=payload<LeaseUpdate>(reply.payload);validateModel(update,'RemoteAutomationWriteLeaseUpdate');this.update(update);}
  catch(error){if(this.held(lease,generation)&&!lease.terminal)this.fail(lease,error instanceof Error?error.message:'The recorded gesture could not be confirmed.',error);}
 }
 update(update:LeaseUpdate){validateModel(update,'RemoteAutomationWriteLeaseUpdate');const lease=this.lease;if(!lease||update.gestureID.toUpperCase()!==lease.gestureID.toUpperCase()||lease.epoch!==this.environment.epoch())return false;
  if(lease.terminal&&lease.terminal.phase!==update.phase)return false;
  if(update.lastAppliedSequence!=null&&update.lastAppliedSequence>lease.sequence)throw Error('Invalid live automation owner sequence.');
  const foreignEpoch=!!update.stamp&&!!lease.initial&&update.stamp.transportEpoch!==lease.initial.stamp.transportEpoch;
  if(foreignEpoch&&['writing','finalizing','committed'].includes(update.phase))throw Error('Invalid live automation owner epoch.');
  // A restarted clock may end the lease, but its stamp is not part of the saved prefix; keep the last original-epoch stamp.
  lease.reason=update.reason||lease.reason;lease.lastAppliedSequence=Math.max(lease.lastAppliedSequence,update.lastAppliedSequence??0);if(!foreignEpoch)lease.stamp=update.stamp??lease.stamp;lease.draftID=update.draftID??lease.draftID;
  if(update.phase==='writing')return true;
  this.stopRenew();lease.ending=true;lease.pending=undefined;
  if(update.phase==='finalizing'){lease.hostFinalizing=true;lease.phase='finalizing';}
  else {lease.terminal=update;lease.revision=update.revision??undefined;lease.phase=update.phase==='committed'?'finalizing':update.phase==='cancelled'?'cancelled':'review';this.remember(lease);}
  this.metadataChanged();this.changed();return true;
 }
 metadataChanged(){const lease=this.lease;if(!lease)return;
  if(lease.epoch!==this.environment.epoch()||!this.environment.available()){this.disconnected();return;}
  if(lease.terminal?.phase==='committed'){const observed=this.environment.version(lease.trackID),revision=lease.terminal.revision!;if(observed===revision&&lease.phase!=='committed'){lease.phase='committed';lease.reason='Recorded on the server. Undo restores the previous curve.';this.remember(lease);}else if(observed!==undefined&&observed>revision&&lease.phase!=='review'){lease.phase='review';lease.reason='The gesture was recorded, then this track changed again. Review the current curve.';this.remember(lease);}return;}
  if(lease.phase==='writing'&& !this.environment.track(lease.trackID))this.fail(lease,'The source track was removed. Review the recorded draft.',undefined,true);
 }
 disconnected(){const lease=this.lease;this.stopRenew();if(lease&&['beginning','writing','finalizing'].includes(lease.phase)){lease.phase='uncertain';lease.reason='Connection lost. Read is restored by the server; review this gesture before retrying.';lease.pending=undefined;lease.ending=true;this.remember(lease);this.changed();}this.generation++;}
 async draft(outcome:WriteOutcome):Promise<DraftReply>{
  if(!this.environment.available())throw Error('Reconnect to the same server and client to review the draft.');
  if(outcome.serverID!==this.environment.serverID?.()||outcome.credentialID!==this.environment.credentialID?.()||outcome.runtimeID!==this.environment.runtimeID?.())throw Error('Reconnect with the same server and client identity to review this draft.');
  const epoch=this.environment.epoch();const reply=await this.environment.send(writeCommand('automation.writeDraft',outcome.draftID?{draftID:outcome.draftID}:{gestureID:outcome.gestureID}));const result=payload<DraftReply>(reply.payload);validateModel(result,'RemoteAutomationWriteDraftReply');
  if(epoch!==this.environment.epoch())throw Error('The connection changed during draft review.');
  if(result.gestureID.toUpperCase()!==outcome.gestureID.toUpperCase()||result.trackID.toUpperCase()!==outcome.trackID.toUpperCase()||!sameWriteTarget(result.target,outcome.target))throw Error('The draft does not belong to this recorded gesture.');return result;
 }
}
