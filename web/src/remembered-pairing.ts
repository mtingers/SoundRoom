import {validateCredential,type Credential} from './sdk.js';
export interface RememberedPairing {endpoint:string;name:string;credential:Credential}
const key='soundroom.trusted-lan-pairing.v1';
export function loadPairing(storage:Pick<Storage,'getItem'|'removeItem'>,origin:string):RememberedPairing|undefined {
 try {const raw=storage.getItem(key);if(!raw)return; if(raw.length>16384)throw Error();const value=JSON.parse(raw);const endpoint=new URL(value.endpoint);if(endpoint.protocol!=='ws:'||endpoint.origin.replace(/^ws:/,'http:')!==origin||endpoint.pathname!=='/control'||endpoint.username||endpoint.password||endpoint.search||endpoint.hash||typeof value.name!=='string'||!value.name.trim()||new TextEncoder().encode(value.name).length>128||/[\u0000-\u001f\u007f]/.test(value.name))throw Error();return {endpoint:endpoint.href,name:value.name,credential:validateCredential(value.credential)};}catch{try{storage.removeItem(key);}catch{}return;}
}
export function savePairing(storage:Pick<Storage,'setItem'>,value:RememberedPairing):void {storage.setItem(key,JSON.stringify({...value,credential:validateCredential(value.credential)}));}
export function forgetPairing(storage:Pick<Storage,'removeItem'>):void {storage.removeItem(key);}
