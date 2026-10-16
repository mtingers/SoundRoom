import {randomUUID} from './crypto.js';
import {SoundroomClient,encodeBase64,decodeBase64} from './sdk.js';
import {SHA256} from './sha256.js';
const CHUNK=256*1024;
const yieldTurn=()=>new Promise<void>(resolve=>setTimeout(resolve,0));
function assertActive(signal?:AbortSignal){if(signal?.aborted)throw new DOMException('Transfer cancelled.','AbortError');}
export async function importAudio(client:SoundroomClient,file:File,trackID:string|undefined,frame:number,onProgress:(progress:number,message:string)=>void,signal?:AbortSignal) {
  if(file.size<=0||file.size>2*1024*1024*1024)throw new Error('Choose an audio file between 1 byte and 2 GiB.');
  const hash=new SHA256();
  for(let offset=0;offset<file.size;offset+=CHUNK){assertActive(signal);hash.update(new Uint8Array(await file.slice(offset,offset+CHUNK).arrayBuffer()));onProgress(0.2*Math.min(1,(offset+CHUNK)/file.size),'Checking file');await yieldTurn();}
  const id=randomUUID().toUpperCase();await client.command({type:'beginUpload',id,name:file.name,bytes:file.size,sha256:hash.digest()});
  for(let offset=0;offset<file.size;offset+=CHUNK){assertActive(signal);const data=encodeBase64(new Uint8Array(await file.slice(offset,offset+CHUNK).arrayBuffer()));await client.command({type:'uploadChunk',id,offset,data});onProgress(0.2+0.75*Math.min(1,(offset+CHUNK)/file.size),'Uploading to server');}
  assertActive(signal);await client.command({type:'finishUpload',id,trackID,frame});onProgress(1,'Imported');
}
export function payload<T>(value:unknown):T {if(typeof value!=='string')throw new Error('Missing server payload.');return JSON.parse(new TextDecoder().decode(decodeBase64(value))) as T;}
interface Artifact {id:string;name:string;bytes:number;sha256:string}
interface ArtifactChunk {offset:number;data:string;complete:boolean}
export interface ExportSink {write(data:Uint8Array):Promise<void>;close():Promise<void>;abort(reason?:unknown):Promise<void>}
export async function downloadMix(client:SoundroomClient,name:string,onProgress:(progress:number,message:string)=>void,signal?:AbortSignal,sink?:ExportSink):Promise<{blob?:Blob;name:string;written:boolean}> {
 try{
  const result=await client.command({type:'exportMix',name}),artifact=payload<Artifact>(result.payload);
  if(!Number.isSafeInteger(artifact.bytes)||artifact.bytes<0||artifact.bytes>2*1024*1024*1024||typeof artifact.id!=='string'||typeof artifact.name!=='string'||!/^[0-9a-f]{64}$/i.test(artifact.sha256))throw new Error('Invalid export artifact.');
  if(!sink&&artifact.bytes>256*1024*1024)throw new Error('This browser cannot stream to disk. Use Chrome/Edge’s save-file workflow or a native client for exports above 256 MiB.');
  const parts:ArrayBuffer[]=[],hash=new SHA256();let offset=0;
  while(offset<artifact.bytes){assertActive(signal);const reply=await client.command({type:'downloadArtifact',id:artifact.id,offset,bytes:Math.min(CHUNK,artifact.bytes-offset)}),chunk=payload<ArtifactChunk>(reply.payload),bytes=decodeBase64(chunk.data);if(chunk.offset!==offset||bytes.length===0||bytes.length>CHUNK||offset+bytes.length>artifact.bytes)throw new Error('Invalid export chunk.');hash.update(bytes);if(sink)await sink.write(bytes);else parts.push(bytes.buffer as ArrayBuffer);offset+=bytes.length;onProgress(offset/artifact.bytes,'Downloading export');}
  assertActive(signal);if(hash.digest()!==artifact.sha256.toLowerCase())throw new Error('Export integrity check failed.');if(sink)await sink.close();return {blob:sink?undefined:new Blob(parts,{type:'audio/wav'}),name:artifact.name,written:!!sink};
 }catch(error){if(sink)await sink.abort(error).catch(()=>{});throw error;}
}
