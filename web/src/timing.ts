import {type Timing} from './model.js';
export function beatFrames(t: Timing, sampleRate: number) { return sampleRate * 60 / t.tempo * 4 / t.beatUnit; }
export function snapFrame(frame: number, t: Timing, sampleRate: number, division = 1) { const interval = beatFrames(t,sampleRate)/division; return Math.max(0, Math.round(Math.round(frame/interval)*interval)); }
export function formatPosition(frame: number, t: Timing, sampleRate: number, timebase: 'time' | 'beats') {
  if (timebase === 'time') { const seconds = frame/sampleRate; return `${Math.floor(seconds/60).toString().padStart(2,'0')}:${(seconds%60).toFixed(3).padStart(6,'0')}`; }
  const ticks = Math.round(frame/sampleRate*t.tempo/60*960), beatTicks = 960*4/t.beatUnit, barTicks = beatTicks*t.beatsPerBar;
  return `${Math.floor(ticks/barTicks)+1}.${Math.floor(ticks%barTicks/beatTicks)+1}.${(ticks%beatTicks).toString().padStart(3,'0')}`;
}
export function parsePosition(value: string, t: Timing, sampleRate: number, timebase: 'time' | 'beats'): number {
  if (timebase === 'beats') { const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(value); if (!match) throw new Error('Use bar.beat.tick, for example 2.1.000.'); const bar=Number(match[1]),beat=Number(match[2]),tick=Number(match[3]),bt=960*4/t.beatUnit; if (bar<1 || bar>1000000 || beat<1 || beat>t.beatsPerBar || tick<0 || tick>=bt) throw new Error('Position is outside this meter.'); return Math.round(((bar-1)*t.beatsPerBar*bt+(beat-1)*bt+tick)/960*60/t.tempo*sampleRate); }
  if (!/^(?:\d+:)?\d+(?:\.\d+)?$/.test(value)) throw new Error('Use seconds or minutes:seconds.'); const parts=value.split(':').map(Number); const seconds=parts.length===2 ? parts[0]! *60+parts[1]! : parts[0]!; if (!Number.isFinite(seconds)) throw new Error('Invalid time.'); const frame=Math.round(seconds*sampleRate); if (!Number.isSafeInteger(frame)) throw new Error('Time exceeds the supported range.'); return frame;
}
export interface Tick { frame: number; label: string; major: boolean }
export function rulerTicks(start: number, end: number, pixelsPerSecond: number, timing: Timing, sampleRate: number, timebase: 'time' | 'beats'): Tick[] {
  const ticks: Tick[]=[];
  let interval: number;
  if (timebase==='beats') { interval=beatFrames(timing,sampleRate); const pixels=interval/sampleRate*pixelsPerSecond; if (pixels<45) interval*=timing.beatsPerBar*Math.max(1,Math.ceil(45/(pixels*timing.beatsPerBar))); }
  else { const candidates=[0.1,0.25,0.5,1,2,5,10,15,30,60,120,300,600,1800,3600]; interval=(candidates.find(s=>s*pixelsPerSecond>=70) ?? 3600)*sampleRate; }
  for(let index=Math.ceil(start/interval),frame=Math.round(index*interval); frame<=end && ticks.length<512; index++,frame=Math.round(index*interval)) { const major=timebase==='time'||Math.abs(frame/beatFrames(timing,sampleRate)/timing.beatsPerBar-Math.round(frame/beatFrames(timing,sampleRate)/timing.beatsPerBar))<0.0001; ticks.push({frame,label:formatPosition(frame,timing,sampleRate,timebase),major}); }
  return ticks;
}
export function edgeScrollVelocity(x: number, width: number): number { const edge=36; return x<edge ? -900*Math.min(1,(edge-x)/edge) : x>width-edge ? 900*Math.min(1,(x-width+edge)/edge) : 0; }
export function zoomAround(oldScale: number,newScale: number,offset: number,anchor: number) { return Math.max(0,(offset+anchor)/oldScale*newScale-anchor); }
