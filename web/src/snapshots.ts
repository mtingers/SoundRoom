import {decodeBase64} from './sdk.js';
import {SHA256} from './sha256.js';
/** One bounded, revision-pinned assembly; partial metadata is never installed. */
export class SnapshotAssembler {
 private active?:{id:string;revision:number;bytes:number;sha256:string;offset:number;chunks:Uint8Array[];hash:SHA256};
 begin(message:Record<string,unknown>){if(typeof message.transferID!=='string'||!Number.isSafeInteger(message.revision)||!Number.isSafeInteger(message.totalBytes)||Number(message.totalBytes)<=0||Number(message.totalBytes)>64*1024*1024||typeof message.sha256!=='string'||!/^[a-f0-9]{64}$/i.test(message.sha256))throw new Error('Invalid paged snapshot.');this.active={id:message.transferID,revision:Number(message.revision),bytes:Number(message.totalBytes),sha256:message.sha256,offset:0,chunks:[],hash:new SHA256()};}
 page(message:Record<string,unknown>){const a=this.active;if(!a||message.transferID!==a.id||message.revision!==a.revision||message.offset!==a.offset||typeof message.data!=='string')throw new Error('Snapshot page is out of order.');const bytes=decodeBase64(message.data);if(bytes.length===0||bytes.length>512*1024||a.offset+bytes.length>a.bytes)throw new Error('Snapshot page exceeds its bounds.');a.hash.update(bytes);a.chunks.push(bytes);a.offset+=bytes.length;}
 end(message:Record<string,unknown>):unknown {const a=this.active;this.active=undefined;if(!a||message.transferID!==a.id||message.revision!==a.revision||message.totalBytes!==a.bytes||message.sha256!==a.sha256||a.offset!==a.bytes||a.hash.digest()!==a.sha256.toLowerCase())throw new Error('Snapshot integrity check failed.');const bytes=new Uint8Array(a.bytes);let offset=0;for(const chunk of a.chunks){bytes.set(chunk,offset);offset+=chunk.length;}return JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(bytes));}
 cancel(){this.active=undefined;}
 get assembling(){return !!this.active;}
}
