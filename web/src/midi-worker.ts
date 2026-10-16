import {inspectMIDI} from './midi.js';
self.addEventListener('message',event=>{const {id,buffer}=(event as MessageEvent).data;try{self.postMessage({id,content:inspectMIDI(buffer)});}catch(error){self.postMessage({id,error:error instanceof Error?error.message:'MIDI import failed.'});}});
