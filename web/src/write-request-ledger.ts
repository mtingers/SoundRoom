import type {Reply} from './model.js';
export interface WriteRequestOwner {serverID:string;credentialID:string;runtimeID:string;connectionEpoch:number;}
export interface RetainedWriteRequest {id:string;frame:string;owner:WriteRequestOwner;phase:'queued'|'sent'|'unsent'|'uncertain'|'known';expiresAt:number;reply?:Reply;}
const sameOwner=(a:WriteRequestOwner,b:WriteRequestOwner,epoch=true)=>a.serverID.toUpperCase()===b.serverID.toUpperCase()&&a.credentialID.toUpperCase()===b.credentialID.toUpperCase()&&a.runtimeID.toUpperCase()===b.runtimeID.toUpperCase()&&(!epoch||a.connectionEpoch===b.connectionEpoch);
/** Bounded live-Write identity retention; it provides no replay facility or restart recovery. */
export class WriteRequestLedger {
 private records=new Map<string,RetainedWriteRequest>();private bytes=0;
 constructor(private now:()=>number=Date.now){}
 private prune(){for(const [id,record]of this.records)if(record.expiresAt<=this.now()){this.bytes-=new TextEncoder().encode(record.frame).length;this.records.delete(id);}}
 reserve(id:string,frame:string,owner:WriteRequestOwner,terminal=false){this.prune();const key=id.toUpperCase(),before=this.records.get(key),bytes=new TextEncoder().encode(frame).length;
  if(before)throw Error(before.frame!==frame||!sameOwner(before.owner,owner)?'A retained request ID cannot identify different content.':'This request identity is already retained.');
  if(bytes>1024||this.records.size>=(terminal?16:13)||this.bytes+bytes>32768)throw Error('Review previous live automation outcomes before recording another gesture.');
  this.records.set(key,{id:key,frame,owner:{...owner},phase:'queued',expiresAt:this.now()+1200000});this.bytes+=bytes;
 }
 sent(id:string){const record=this.records.get(id.toUpperCase());if(record?.phase==='queued')record.phase='sent';}
 unsent(id:string){const record=this.records.get(id.toUpperCase());if(record?.phase==='queued')record.phase='unsent';}
 uncertain(id:string){const record=this.records.get(id.toUpperCase());if(record?.phase==='sent'){record.phase='uncertain';record.expiresAt=this.now()+600000;}}
 receipt(reply:Reply,owner:WriteRequestOwner){this.prune();const key=reply.requestID.toUpperCase(),record=this.records.get(key);if(!record||!sameOwner(record.owner,owner)||record.phase==='queued'||record.phase==='unsent')return false;
  if(record.phase==='uncertain'){record.phase='known';record.reply={...reply};return true;}
  if(record.phase==='sent'){this.bytes-=new TextEncoder().encode(record.frame).length;this.records.delete(key);}return false;
 }
 view(owner:WriteRequestOwner){this.prune();return [...this.records.values()].filter(r=>sameOwner(r.owner,owner,false)).map(r=>({...r,owner:{...r.owner},reply:r.reply?{...r.reply}:undefined}));}
 forget(id:string,owner:WriteRequestOwner){this.prune();const key=id.toUpperCase(),record=this.records.get(key);if(!record||!sameOwner(record.owner,owner,false)||['queued','sent'].includes(record.phase))return false;this.bytes-=new TextEncoder().encode(record.frame).length;return this.records.delete(key);}
 get retainedBytes(){this.prune();return this.bytes;}
}
