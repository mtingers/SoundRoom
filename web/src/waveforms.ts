import {type Asset} from './model.js';
export class WaveformCache {
  private entries=new Map<string,{asset:Asset;bytes:number}>(); private used=0;
  constructor(readonly budget=8*1024*1024){}
  put(asset:Asset) { if(!asset.peaks.length)return;const key=`${asset.id}:${asset.waveformStartFrame}:${asset.framesPerBin??Math.ceil(asset.frames/(asset.peaks.length/2))}`,bytes=asset.peaks.length*8; const old=this.entries.get(key);if(old)this.used-=old.bytes;this.entries.delete(key);if(bytes>this.budget)return;this.entries.set(key,{asset,bytes});this.used+=bytes;while(this.used>this.budget){const key=this.entries.keys().next().value as string;this.used-=this.entries.get(key)!.bytes;this.entries.delete(key);}}
  covering(id:string,start:number,end:number):Asset|undefined {let best:Asset|undefined;for(const {asset} of this.entries.values()){if(asset.id!==id)continue;const bin=asset.framesPerBin??Math.ceil(asset.frames/(asset.peaks.length/2));if(asset.waveformStartFrame<=start&&asset.waveformStartFrame+asset.peaks.length/2*bin>=end&&(!best||bin<(best.framesPerBin??Math.ceil(best.frames/(best.peaks.length/2)))))best=asset;}return best;}
  clear(){this.entries.clear();this.used=0;}
  get bytes(){return this.used;}
}
/** Network waveform reads are coalesced per asset and never flood the control lane. */
export class WaveformLoader {
 private waiting=new Map<string,{id:string;start:number;end:number;points:number}>();private active=0;private generation=0;
 constructor(private load:(id:string,start:number,end:number,points:number)=>Promise<Asset>,private receive:(asset:Asset)=>void,private report:(message:string)=>void){}
 request(id:string,start:number,end:number,points:number,overview=false){const key=`${id}:${overview?'overview':'detail'}`;this.waiting.delete(key);this.waiting.set(key,{id,start,end,points});while(this.waiting.size>128)this.waiting.delete(this.waiting.keys().next().value!);this.drain();}
 private drain(){while(this.active<2&&this.waiting.size){const key=this.waiting.keys().next().value!,task=this.waiting.get(key)!;this.waiting.delete(key);this.active++;const generation=this.generation;void this.load(task.id,task.start,task.end,task.points).then(asset=>{if(generation===this.generation)this.receive(asset);}).catch(error=>{if(generation===this.generation)this.report(error instanceof Error?error.message:'Waveform unavailable.');}).finally(()=>{this.active--;this.drain();});}}
 reset(){this.waiting.clear();this.generation++;}
 get pending(){return this.waiting.size;}
 get inFlight(){return this.active;}
}
