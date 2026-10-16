import {randomUUID, hmacSHA256} from './crypto.js';
import {validateRequest,validateModel} from './validate.js';
import {WriteRequestLedger,type WriteRequestOwner} from './write-request-ledger.js';
import {eventFields} from './generated/contract.js';
import {type Command,type Reply,type Peer} from './model.js';
export interface Credential { version: number; serverID: string; clientID: string; credentialID?: string; clientSecret: string; role: Peer['role'] }
export type ConnectionState = 'disconnected' | 'connecting' | 'authenticating' | 'connected' | 'reconnecting' | 'failed';
export interface SocketLike { readyState: number; bufferedAmount: number; send(data: string): void; close(code?:number,reason?:string): void; onopen: ((event:Event)=>void)|null; onmessage: ((event:{data:unknown})=>void)|null; onclose: ((event:unknown)=>void)|null; onerror: ((event:unknown)=>void)|null }
export class CommandError extends Error { constructor(readonly reply: Reply) { super(reply.message || reply.status); } }
export class UncertainCommandError extends Error { constructor(readonly requestID: string) { super(`Connection lost. Request ${requestID} may have been applied; reconcile before retrying.`); } }
export function decodeBase64(value: string): Uint8Array { const raw=atob(value); return Uint8Array.from(raw,c=>c.charCodeAt(0)); }
export function encodeBase64(bytes: Uint8Array): string { let value=''; for(let i=0;i<bytes.length;i+=8192) value+=String.fromCharCode(...bytes.subarray(i,i+8192)); return btoa(value); }
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function validateCredential(value: unknown): Credential {
  if(!value || typeof value!=='object' || Array.isArray(value))throw new Error('Invalid Soundroom pairing file.');
  const source=value as Credential, c={...source,clientID:source.clientID??source.credentialID}; if (!c || ![1,3].includes(c.version) || !uuid.test(c.serverID) || !uuid.test(c.clientID??'') || !['viewer','editor','operator'].includes(c.role) || typeof c.clientSecret!=='string' || decodeBase64(c.clientSecret).length!==32) throw new Error('Invalid Soundroom pairing file.'); return {version:c.version,serverID:c.serverID,clientID:c.clientID!,credentialID:c.credentialID,clientSecret:c.clientSecret,role:c.role};
}
export async function authenticationProof(c: Credential,peer: Peer,nonce: string): Promise<string> {
  if (decodeBase64(nonce).length!==32) throw new Error('Invalid authentication challenge.');
  const text=`soundroom-auth-v2|${c.serverID.toUpperCase()}|${c.clientID.toUpperCase()}|${peer.id.toUpperCase()}|${peer.name}|${nonce}`;
  if (!crypto.subtle) return encodeBase64(hmacSHA256(decodeBase64(c.clientSecret),new TextEncoder().encode(text)));
  const key=await crypto.subtle.importKey('raw',decodeBase64(c.clientSecret).buffer as ArrayBuffer,{name:'HMAC',hash:'SHA-256'},false,['sign']);
  return encodeBase64(new Uint8Array(await crypto.subtle.sign('HMAC',key,new TextEncoder().encode(text))));
}
/** Send only features explicitly advertised by this host and understood by this client. */
export function clientFeatureDeclaration(capabilities:Record<string,unknown>):Command|undefined {
  if(!Array.isArray(capabilities.commands)||!capabilities.commands.includes('editor')||!Array.isArray(capabilities.editorOperations)||!capabilities.editorOperations.includes('client.features')||!Array.isArray(capabilities.features)||!capabilities.features.includes('compact-control-patches'))return;
  return {type:'editor',intent:{operation:'client.features',context:{cursorFrame:0,clipIDs:[]},integers:{},numbers:{},strings:{},flags:{},payload:encodeBase64(new TextEncoder().encode(JSON.stringify(['compact-control-patches'])))}};
}
interface Pending { resolve:(reply:Reply)=>void; reject:(error:Error)=>void; timer:ReturnType<typeof setTimeout>; command:Command }
export class SoundroomClient extends EventTarget {
  state:ConnectionState='disconnected'; role:Peer['role']='viewer'; serverName=''; clientID=randomUUID().toUpperCase();
  private featureDeclarationGeneration?:number;
  private socket?:SocketLike; private pending=new Map<string,Pending>(); private generation=0; private reconnectTimer?:ReturnType<typeof setTimeout>; private intentional=false; private attempt=0; private pairedCredential?:Credential; private challenge?:{serverID:string;nonce:string}; public capabilities:Record<string,unknown>={}; private authenticatedHello=false; private handshake:'challenge'|'pairing'|'hello'|'complete'='challenge';
  private writeLedger=new WriteRequestLedger();
  private writeOwner():WriteRequestOwner|undefined {const credential=this.pairedCredential;return credential?{serverID:credential.serverID,credentialID:credential.credentialID??credential.clientID,runtimeID:this.clientID,connectionEpoch:this.generation}:undefined;}
  get credentialIdentity(){return this.pairedCredential?.credentialID??this.pairedCredential?.clientID;}
  get retainedAutomationRequests(){const owner=this.writeOwner();return owner?this.writeLedger.view(owner):[];}
  forgetAutomationRequest(id:string){const owner=this.writeOwner();return !!owner&&this.writeLedger.forget(id,owner);}
  get connectionEpoch(){return this.generation;}
  constructor(private factory:(url:string)=>SocketLike = url=>new WebSocket(url,'soundroom.v3') as unknown as SocketLike) { super(); }
  private trustedLANOrigin?: string;
  private connectionFailure?: string;
  private connectionURL='';
  allowTrustedLAN(origin:string) {
    const parsed=new URL(origin);
    if(parsed.protocol!=='http:' || parsed.origin!==origin || parsed.username || parsed.password) throw new Error('Trusted LAN requires an exact HTTP origin.');
    this.trustedLANOrigin=origin;
  }
  private stateChanged(state:ConnectionState,message='') { this.state=state; this.dispatchEvent(new CustomEvent('connection',{detail:{state,message}})); }
  async connect(url:string,credential:Credential|{code:string},name='Browser',retry=false):Promise<void> {
    if(!retry)this.attempt=0;
    this.connectionFailure=undefined;
    const endpoint=new URL(url); const trustedLAN=endpoint.protocol==='ws:' && this.trustedLANOrigin===endpoint.origin.replace(/^ws:/,'http:') && endpoint.pathname==='/control'; if((endpoint.protocol!=='wss:' && !trustedLAN) || endpoint.username || endpoint.password || endpoint.search || endpoint.hash) throw new Error('Use a secure WSS endpoint without credentials or query parameters.');
    if('code' in credential && !/^[A-Za-z0-9_-]{43}$/.test(credential.code))throw new Error('Enter the pairing code shown by the server.'); name=name.trim();if(!name||/[\u0000-\u001f\u007f]/.test(name)||new TextEncoder().encode(name).length>128)throw new Error('Client name must be 1–128 UTF-8 bytes without control characters.');if(!('code' in credential)){this.pairedCredential=validateCredential(credential);credential=this.pairedCredential;}else this.pairedCredential=undefined;this.capabilities={};this.challenge=undefined;this.handshake='challenge';for(const [id,p]of this.pending){clearTimeout(p.timer);this.writeLedger.uncertain(id);p.reject(new UncertainCommandError(id));}this.pending.clear();clearTimeout(this.reconnectTimer); this.intentional=false; this.authenticatedHello=false; const generation=++this.generation; this.socket?.close(1000,'Reconnect'); this.stateChanged(this.attempt?'reconnecting':'connecting');
    this.connectionURL=endpoint.href;
    const socket=this.factory(endpoint.href); this.socket=socket;
    socket.onopen=()=>{ if(generation===this.generation) this.stateChanged('authenticating'); };
    socket.onmessage=event=>{ if(generation!==this.generation||this.intentional) return; void this.receive(event.data,socket,credential,name,generation).catch(error=>{ if(generation!==this.generation||this.intentional)return; this.connectionFailure=error instanceof Error?error.message:'Invalid server frame.'; this.stateChanged('failed',this.connectionFailure); this.intentional=true; socket.close(4008,'Invalid server frame'); }); };
    socket.onerror=()=>{ if(generation===this.generation){this.connectionFailure='Connection failed. Check the server address and availability; WSS also requires certificate trust.';if(!this.attempt)this.stateChanged('failed',this.connectionFailure);} };
    socket.onclose=()=>{
      if(generation!==this.generation) return;
      for(const [id,p] of this.pending) { clearTimeout(p.timer); this.writeLedger.uncertain(id);p.reject(new UncertainCommandError(id)); } this.pending.clear();
      if(this.connectionFailure && this.intentional) this.stateChanged('failed',this.connectionFailure);
      else if(this.intentional) this.stateChanged('disconnected');
      // A first connection that never authenticates fails at once; an outage
      // after a session was established keeps its retry budget even when a
      // retry socket is refused before authentication (e.g. server restart).
      else if((this.handshake!=='complete' && this.attempt===0) || this.attempt>=3) {
        this.intentional=true;
        this.stateChanged('failed',this.connectionFailure??'Connection closed before authentication completed. Check the server, then click Connect to retry.');
      } else {
        this.stateChanged('reconnecting','Connection lost. Retrying; edits remain disabled until a fresh snapshot arrives.');
        const delay=500*2**this.attempt++;
        this.reconnectTimer=setTimeout(()=>{void this.connect(url,this.pairedCredential??credential,name,true).catch(error=>{this.intentional=true;this.connectionFailure=error instanceof Error?error.message:'Reconnect failed.';this.stateChanged('failed',this.connectionFailure);});},delay);
      }
    };
  }
  forgetCredential() {this.disconnect();this.pairedCredential=undefined;}
  disconnect() { this.connectionFailure=undefined; this.intentional=true; clearTimeout(this.reconnectTimer); this.socket?.close(1000,'Client detached'); }
  private async receive(raw:unknown,socket:SocketLike,credential:Credential|{code:string},name:string,generation:number) {
    if(typeof raw!=='string' || new TextEncoder().encode(raw).byteLength>8*1024*1024) throw new Error('Oversized or invalid server frame.');
    const message=JSON.parse(raw) as Record<string,unknown>; if(message.version!==3 || typeof message.kind!=='string') throw new Error('Incompatible protocol version.');
    if(message.kind==='challenge') {
      if(this.handshake!=='challenge' || typeof message.serverID!=='string' || !uuid.test(message.serverID) || typeof message.nonce!=='string' || decodeBase64(message.nonce).length!==32) throw new Error('Invalid server challenge.');
      this.challenge={serverID:message.serverID,nonce:message.nonce};
      if('code' in credential){this.handshake='pairing';socket.send(JSON.stringify({version:3,kind:'pair',code:credential.code,name}));return;}
      await this.sendHello(socket,credential,name,generation); return;
    }
    if(message.kind==='paired') {if(this.handshake!=='pairing')throw new Error('Unexpected pairing credential.');const c=validateCredential(message.credential);this.pairedCredential=c;await this.sendHello(socket,c,name,generation);return;}
    if(message.kind==='welcome') { if(this.handshake!=='hello' || !this.authenticatedHello || typeof message.serverID!=='string' || message.serverID.toUpperCase()!==this.pairedCredential?.serverID.toUpperCase())throw new Error('Unauthenticated server welcome.'); if(!['viewer','editor','operator'].includes(String(message.role))) throw new Error('Invalid server role.'); if(typeof message.name!=='string'||new TextEncoder().encode(message.name).length>1024)throw new Error('Invalid server name.');this.handshake='complete';this.attempt=0;this.connectionFailure=undefined;this.role=message.role as Peer['role']; this.serverName=message.name; if(message.capabilities)validateModel(message.capabilities,'RemoteCapabilities');this.capabilities=(message.capabilities as Record<string,unknown>)??{}; this.stateChanged('connected'); this.dispatchEvent(new CustomEvent('authenticatedcredential',{detail:{credential:{...this.pairedCredential!},endpoint:this.connectionURL,name}})); this.dispatchEvent(new CustomEvent('welcome',{detail:message})); this.negotiateFeatures(generation); await this.command({type:'snapshot'}); return; }
    // Heartbeats can arrive during a slow authenticated pairing exchange.
    if(message.kind==='heartbeat'){if(typeof message.id!=='string'||!uuid.test(message.id))throw new Error('Invalid heartbeat.');socket.send(JSON.stringify({version:3,kind:'heartbeatReply',id:message.id}));return;}
    if(message.kind==='failure')throw new Error(String(message.message??'Authentication failed.'));
    if(this.handshake!=='complete')throw new Error('Application frame before authenticated welcome.');
    if(message.kind==='capabilities'){if(!message.capabilities||typeof message.capabilities!=='object')throw new Error('Invalid capabilities.');validateModel(message.capabilities,'RemoteCapabilities');this.capabilities=message.capabilities as Record<string,unknown>;this.dispatchEvent(new CustomEvent('capabilities',{detail:this.capabilities}));this.negotiateFeatures(generation);return;}
    if(message.kind==='reply') { const reply=message as unknown as Reply; if(typeof reply.requestID!=='string' || !uuid.test(reply.requestID) || !['accepted','rejected','conflict','busy'].includes(reply.status) || !Number.isSafeInteger(reply.revision)||reply.revision<0||typeof reply.message!=='string'||reply.payload!==undefined&&typeof reply.payload!=='string') throw new Error('Invalid receipt.'); const owner=this.writeOwner();if(owner&&this.writeLedger.receipt(reply,owner))this.dispatchEvent(new CustomEvent('automationreceipt',{detail:reply}));const p=this.pending.get(reply.requestID.toUpperCase()); if(p) { clearTimeout(p.timer); this.pending.delete(reply.requestID.toUpperCase()); if(reply.status==='accepted') p.resolve(reply); else { const error=new CommandError(reply); p.reject(error); this.dispatchEvent(new CustomEvent('conflict',{detail:reply})); } } return; }
    if(['snapshotBegin','snapshotPage','snapshotEnd'].includes(message.kind)){this.dispatchEvent(new CustomEvent('snapshotpage',{detail:message}));return;}
    if(message.kind==='event') { const event=message.event as Record<string,unknown>; if(!event || typeof event.type!=='string' || !Object.hasOwn(eventFields,event.type)) throw new Error('Invalid session event.');const fields=eventFields[event.type as keyof typeof eventFields];if(Object.keys(event).some(k=>k!=='type'&&!fields.some(f=>f.name===k)))throw new Error('Invalid event fields.');for(const field of fields){const value=event[field.name];if(value===undefined||value===null){if(!field.optional)throw new Error('Missing event field.');}else validateModel(value,field.type);} this.dispatchEvent(new CustomEvent('serverevent',{detail:event})); return; }
    if(message.kind==='heartbeatReply'){if(typeof message.id!=='string'||!uuid.test(message.id))throw new Error('Invalid heartbeat.');return;}
    throw new Error('Unsupported server frame.');
  }
  private negotiateFeatures(generation:number) {
    const declaration=clientFeatureDeclaration(this.capabilities);
    if(!declaration||this.featureDeclarationGeneration===generation||generation!==this.generation||this.handshake!=='complete')return;
    this.featureDeclarationGeneration=generation;
    void this.command(declaration).catch(error=>{if(generation===this.generation)this.dispatchEvent(new CustomEvent('featureerror',{detail:{message:error instanceof Error?error.message:'Client feature negotiation failed.'}}));});
  }
  private async sendHello(socket:SocketLike,c:Credential,name:string,generation:number){
    const challenge=this.challenge;if(!challenge || challenge.serverID.toUpperCase()!==c.serverID.toUpperCase())throw new Error('The pairing file belongs to another server.');
    this.handshake='hello';const peer:Peer={id:this.clientID,name,role:c.role};const proof=await authenticationProof(c,peer,challenge.nonce);if(generation!==this.generation||this.intentional||socket.readyState!==1)return;
    this.handshake='hello';this.authenticatedHello=true;socket.send(JSON.stringify({version:3,kind:'hello',serverID:c.serverID,peer,credentialID:c.clientID,proof}));
  }
  command(command:Command,expectedRevision?:number,expectedTrackRevisions:Record<string,number>={}):Promise<Reply> {
    if(Array.isArray(this.capabilities.commands)&&!this.capabilities.commands.includes(command.type))return Promise.reject(new Error('This server does not support that command.'));
    if(this.state!=='connected' || !this.socket || this.socket.readyState!==1) return Promise.reject(new Error('Connect to a server before sending commands.'));
    // No unbounded JavaScript send queue; safety commands get reserved capacity.
    const operation=(command.intent as {operation?:string}|undefined)?.operation;const priority=['stop','takeTransport'].includes(command.type)||command.type==='editor'&&['automation.writeValue','automation.writeEnd','audition.stop','transport.pause','transport.stop','transport.finishRecording','designer.noteOn','designer.noteOff','designer.controller','designer.configure','designer.allNotesOff','designer.stop','drums.stop','take.stop','jobs.cancel'].includes(operation??'');
    const heavy=['finishUpload','openProject','saveProject','exportMix'].includes(command.type)||command.type==='editor'&&['automation.writeEnd','automation.writeDraft','effects.apply','pitch.analyze','pitch.render','pitch.apply','pitch.manualApply','master.analyze','master.proposal','master.applyProposal','pitch.targets','clips.combine','designer.commit','drums.commit','drums.export','project.export','project.new'].includes(operation??'');
    if(this.pending.size>=(priority?64:60) || this.socket.bufferedAmount>(priority?4*1024*1024:1024*1024)) return Promise.reject(new Error('Connection is busy. Wait for pending commands.'));
    const id=randomUUID().toUpperCase(), frame=JSON.stringify({version:3,kind:'request',id,clientID:this.clientID,expectedRevision,expectedTrackRevisions,command});
    try{validateRequest(JSON.parse(frame));}catch(error){return Promise.reject(error);}
    if(new TextEncoder().encode(frame).byteLength>8*1024*1024) return Promise.reject(new Error('Command exceeds the frame limit.'));
    const retainedWrite=command.type==='editor'&&['automation.writeBegin','automation.writeValue','automation.writeEnd'].includes(operation??'');
    const owner=this.writeOwner();if(retainedWrite){if(!owner)return Promise.reject(new Error('Authenticate before recording automation.'));try{this.writeLedger.reserve(id,frame,owner,operation!=='automation.writeBegin');}catch(error){return Promise.reject(error);}}
    return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{this.pending.delete(id);this.writeLedger.uncertain(id);reject(new UncertainCommandError(id));},heavy?600000:30000);this.pending.set(id,{resolve,reject,timer,command});try{this.socket!.send(frame);if(retainedWrite)this.writeLedger.sent(id);}catch{clearTimeout(timer);this.pending.delete(id);if(retainedWrite){this.writeLedger.unsent(id);reject(new Error('The automation request was not sent. Check the connection before trying again.'));}else reject(new UncertainCommandError(id));}});
  }
}
