// Node worker_threads adapter for the unmodified browser draft worker module.
import {parentPort} from 'node:worker_threads';
const listeners=[];
globalThis.self={addEventListener:(type,listener)=>{if(type==='message')listeners.push(listener);},postMessage:(value,options)=>parentPort.postMessage(value,options?.transfer??[])};
parentPort.on('message',data=>{for(const listener of listeners)listener({data});});
await import('../../dist/write-draft-worker.js');
