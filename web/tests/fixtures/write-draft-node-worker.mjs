import {parentPort} from 'node:worker_threads';
globalThis.self={addEventListener(type,listener){if(type==='message')parentPort.on('message',data=>listener({data}));},postMessage(data,options){parentPort.postMessage(data,options?.transfer??[]);}};
await import('../../dist/write-draft-worker.js');
