import type {Clip as CanonicalClip,MasteringAnalysis,RemoteTrackControlPatch} from './generated/models.js';
/** Display metadata only. The browser never opens an audio device. */
export type ID = string;
export interface Timing { tempo: number; beatsPerBar: number; beatUnit: number }
export interface Clip extends CanonicalClip {}
export interface Track { id: ID; name: string; color: number; clips: Clip[]; gain: number; pan: number; muted: boolean; solo: boolean; armed: boolean; inputDeviceUID?: string; inputDeviceName?: string; inputChannel?: number; stereoInput?: boolean; realtimeEffects: unknown[]; mixAutomation: Record<string, unknown[]> }
export interface Session { name: string; version: number; timing: Timing; tracks: Track[]; masterTrack: Track;masteringAnalysis?:MasteringAnalysis }
export interface Transport { frame: number; isPlaying: boolean; isRecording: boolean; isPreparing: boolean; hostTime: number; loopStart?: number; loopEnd?: number }
export interface Device { id: number; uid: string; name: string; inputChannels: number; outputChannels: number }
export interface Asset { id: ID; frames: number; channels: number; peaks: number[]; waveformStartFrame: number; framesPerBin?: number }
export interface Peer { id: ID; name: string; role: 'viewer' | 'editor' | 'operator' }
export interface Snapshot { serverID: ID; serverName: string; revision: number; trackRevisions: Record<ID, number>; session: Session; transport: Transport; assets: Asset[]; devices: Device[]; inputDeviceID: number; outputDeviceID: number; peers: Peer[]; status: string; sampleRate?: number }
export interface Delta { controlPatches?: RemoteTrackControlPatch[] | null; revision: number; tracks: Track[]; controls: (Partial<Track> & {trackID: ID})[]; removedTrackIDs: ID[]; trackOrder?: ID[]; timing?: Timing; name?: string; assets: Asset[]; status?: string; trackRevisions: Record<ID, number>;metadataUpdated?:boolean;masteringAnalysis?:MasteringAnalysis;sessionVersion?:number }
export interface Command { type: string; [key: string]: unknown }
export interface Reply { requestID: ID; status: 'accepted' | 'rejected' | 'conflict' | 'busy'; revision: number; message: string; payload?: unknown }
export const SAMPLE_RATE = 48000;
export function validFrame(value: unknown): value is number { return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 }
export function validTiming(value: unknown): value is Timing {
  if (!value || typeof value !== 'object') return false;
  const t = value as Timing;
  return Number.isFinite(t.tempo) && t.tempo >= 20 && t.tempo <= 300 && Number.isInteger(t.beatsPerBar) && t.beatsPerBar >= 1 && t.beatsPerBar <= 16 && [2,4,8,16].includes(t.beatUnit);
}
const UUID=/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i;
const finite=(v:unknown):v is number=>typeof v==='number'&&Number.isFinite(v);
const record=(v:unknown):v is Record<string,unknown>=>!!v&&typeof v==='object'&&!Array.isArray(v);
export function validateTransport(value:unknown):value is Transport{if(!record(value))return false;return validFrame(value.frame)&&typeof value.isPlaying==='boolean'&&typeof value.isRecording==='boolean'&&typeof value.isPreparing==='boolean'&&finite(value.hostTime)&&['loopStart','loopEnd','recordingStartFrame'].every(k=>value[k]===undefined||value[k]===null||validFrame(value[k]))&&(value.loopStart==null&&value.loopEnd==null||validFrame(value.loopStart)&&validFrame(value.loopEnd)&&value.loopEnd>value.loopStart);}
export function validateDevices(value:unknown):value is Device[]{return Array.isArray(value)&&value.length<=4096&&value.every(d=>record(d)&&validFrame(d.id)&&d.id<=4294967295&&typeof d.uid==='string'&&d.uid.length<=4096&&typeof d.name==='string'&&validFrame(d.inputChannels)&&d.inputChannels<=65536&&validFrame(d.outputChannels)&&d.outputChannels<=65536);}
export function validateAsset(value:unknown):value is Asset{if(!record(value))return false;return typeof value.id==='string'&&UUID.test(value.id)&&validFrame(value.frames)&&validFrame(value.channels)&&value.channels>0&&value.channels<=65536&&validFrame(value.waveformStartFrame)&&Array.isArray(value.peaks)&&value.peaks.length<=16384&&value.peaks.length%2===0&&value.peaks.every(finite)&&(value.framesPerBin===undefined||value.framesPerBin===null||validFrame(value.framesPerBin)&&value.framesPerBin>0);}
export function validateTrack(value:unknown):value is Track{if(!record(value))return false;return typeof value.id==='string'&&UUID.test(value.id)&&typeof value.name==='string'&&validFrame(value.color)&&finite(value.gain)&&value.gain>=0&&value.gain<=4&&finite(value.pan)&&value.pan>=-1&&value.pan<=1&&typeof value.armed==='boolean'&&typeof value.muted==='boolean'&&typeof value.solo==='boolean'&&(value.inputChannel===undefined||validFrame(value.inputChannel))&&(value.stereoInput===undefined||typeof value.stereoInput==='boolean')&&Array.isArray(value.realtimeEffects)&&value.realtimeEffects.length<=128&&record(value.mixAutomation)&&Array.isArray(value.clips)&&value.clips.every(c=>record(c)&&typeof c.id==='string'&&UUID.test(c.id)&&typeof c.assetID==='string'&&UUID.test(c.assetID)&&typeof c.name==='string'&&validFrame(c.start)&&validFrame(c.offset)&&validFrame(c.length)&&validFrame(c.start+c.length)&&validFrame(c.offset+c.length));}
function revisions(value:unknown){return record(value)&&Object.entries(value).every(([id,rev])=>UUID.test(id)&&validFrame(rev));}
export function validateSnapshot(value: unknown): value is Snapshot {
 if(!record(value)||!record(value.session))return false;
 return typeof value.serverID==='string'&&UUID.test(value.serverID)&&typeof value.serverName==='string'&&validFrame(value.revision)&&revisions(value.trackRevisions)&&validateTransport(value.transport)&&validateDevices(value.devices)&&validFrame(value.inputDeviceID)&&validFrame(value.outputDeviceID)&&typeof value.status==='string'&&typeof value.session.name==='string'&&validFrame(value.session.version)&&validTiming(value.session.timing)&&Array.isArray(value.session.tracks)&&value.session.tracks.every(validateTrack)&&validateTrack(value.session.masterTrack)&&Array.isArray(value.assets)&&value.assets.every(validateAsset)&&Array.isArray(value.peers)&&value.peers.every(p=>record(p)&&typeof p.id==='string'&&UUID.test(p.id)&&typeof p.name==='string'&&['viewer','editor','operator'].includes(String(p.role)));
}
export function validateControlPatch(value:unknown):value is RemoteTrackControlPatch {
 if(!record(value)||typeof value.trackID!=='string'||!UUID.test(value.trackID)||Object.keys(value).some(k=>!['trackID','gain','pan','muted','solo','armed'].includes(k)))return false;
 return (value.gain==null||finite(value.gain)&&value.gain>=0&&value.gain<=4)&&(value.pan==null||finite(value.pan)&&value.pan>=-1&&value.pan<=1)&&['muted','solo','armed'].every(k=>value[k]==null||typeof value[k]==='boolean')&&['gain','pan','muted','solo','armed'].some(k=>value[k]!=null);
}
function validControlPatchSet(value:unknown):value is RemoteTrackControlPatch[]{
 return Array.isArray(value)&&value.length<=1024&&value.every(validateControlPatch)&&new Set(value.map(p=>p.trackID.toUpperCase())).size===value.length;
}
export function validateDelta(value:unknown):value is Delta{
 if(!record(value))return false;
 return (value.controlPatches==null||validControlPatchSet(value.controlPatches))&&validFrame(value.revision)&&revisions(value.trackRevisions)&&Array.isArray(value.tracks)&&value.tracks.every(validateTrack)&&Array.isArray(value.controls)&&value.controls.every(c=>record(c)&&typeof c.trackID==='string'&&UUID.test(c.trackID)&&finite(c.gain)&&c.gain>=0&&c.gain<=4&&finite(c.pan)&&c.pan>=-1&&c.pan<=1&&Array.isArray(c.realtimeEffects)&&record(c.mixAutomation))&&Array.isArray(value.removedTrackIDs)&&value.removedTrackIDs.every(id=>typeof id==='string'&&UUID.test(id))&&Array.isArray(value.assets)&&value.assets.every(validateAsset)&&(value.timing===undefined||value.timing===null||validTiming(value.timing))&&(value.trackOrder===undefined||value.trackOrder===null||Array.isArray(value.trackOrder)&&value.trackOrder.every(id=>typeof id==='string'&&UUID.test(id)));
}
export class SessionMirror {
  snapshot: Snapshot | undefined;
  /** Navigation is deliberately separate from authoritative metadata. */
  view = { selectedTrack: '', selectedClips: new Set<ID>(), cursor: 0, range: undefined as [number,number] | undefined, offset: 0, pixelsPerSecond: 65, timebase: 'beats' as 'time' | 'beats', snap: true, follow: true };
  transportReceived = 0;
  applySnapshot(snapshot: Snapshot, now: number) { this.snapshot = snapshot; this.transportReceived = now; this.prune(); }
  applyDelta(delta: Delta): boolean {
    const s = this.snapshot;
    if (!s || delta.revision <= s.revision) return false;
    if (delta.revision !== s.revision + 1) throw new Error('Session revision gap; a fresh snapshot is required.');
    if(delta.controlPatches!=null&&!validControlPatchSet(delta.controlPatches))throw new Error('Invalid scalar mixer patch.');
    const patchTargets=new Set([...s.session.tracks, s.session.masterTrack,...delta.tracks].map(t=>t.id.toUpperCase()));
    const patchRemoved=new Set(delta.removedTrackIDs.map(id=>id.toUpperCase()));
    if((delta.controlPatches??[]).some(p=>!patchTargets.has(p.trackID.toUpperCase())||patchRemoved.has(p.trackID.toUpperCase())))throw new Error('Scalar patch target is unavailable.');
    const removed = new Set(delta.removedTrackIDs), replacements = new Map(delta.tracks.map(t => [t.id,t]));
    s.session.tracks = s.session.tracks.filter(t => !removed.has(t.id)).map(t => { const updated = replacements.get(t.id); replacements.delete(t.id); return updated ?? t; });
    const master=replacements.get(s.session.masterTrack.id);if(master){s.session.masterTrack=master;replacements.delete(master.id);}s.session.tracks.push(...replacements.values());
    for (const controls of delta.controls) {
      const t = controls.trackID === s.session.masterTrack.id ? s.session.masterTrack : s.session.tracks.find(t => t.id === controls.trackID);
      if (t) { const {trackID: _id,...values} = controls; for(const key of ['name','color','gain','pan','muted','solo','armed','inputDeviceUID','inputDeviceName','inputChannel','stereoInput','volumeAutomationScale','mixAutomation','realtimeEffects','routing'])if(Object.hasOwn(values,key))(t as unknown as Record<string,unknown>)[key]=(values as Record<string,unknown>)[key]; }
    }
    for (const patch of delta.controlPatches ?? []) {
      const id=patch.trackID.toUpperCase(),t=id===s.session.masterTrack.id.toUpperCase()?s.session.masterTrack:s.session.tracks.find(t=>t.id.toUpperCase()===id);
      if(t)for(const key of ['gain','pan','muted','solo','armed'] as const)if(patch[key]!=null)(t as unknown as Record<string,unknown>)[key]=patch[key];
    }
    if (delta.trackOrder) { const order = new Map(delta.trackOrder.map((id,i) => [id,i])); s.session.tracks.sort((a,b) => (order.get(a.id) ?? Infinity) - (order.get(b.id) ?? Infinity)); }
    if (delta.timing) s.session.timing = delta.timing;
    if(delta.metadataUpdated)s.session.masteringAnalysis=delta.masteringAnalysis;
    if(delta.sessionVersion!==undefined)s.session.version=delta.sessionVersion;
    if (delta.name !== undefined) s.session.name = delta.name;
    if (delta.status !== undefined) s.status = delta.status;
    Object.assign(s.trackRevisions, delta.trackRevisions); s.revision = delta.revision;
    for (const asset of delta.assets) { const i = s.assets.findIndex(a => a.id === asset.id); if (i < 0) s.assets.push(asset); else s.assets[i] = asset; }
    this.prune(); return true;
  }
  updateTransport(transport: Transport, now: number) { if (this.snapshot) this.snapshot.transport = transport; this.transportReceived = now; }
  displayedFrame(now: number): number {
    const t = this.snapshot?.transport; if (!t) return 0;
    // Extrapolation is bounded: stalled telemetry must not pretend playback continues forever.
    let frame = t.frame + (t.isPlaying ? Math.round(Math.min(500, Math.max(0, now-this.transportReceived)) / 1000 * this.sampleRate) : 0);
    if (t.loopStart !== undefined && t.loopEnd !== undefined && t.loopEnd > t.loopStart && frame >= t.loopEnd) frame = t.loopStart + (frame-t.loopStart) % (t.loopEnd-t.loopStart);
    return frame;
  }
  get sampleRate() { return this.snapshot?.sampleRate ?? SAMPLE_RATE; }
  private prune() { const tracks = this.snapshot?.session.tracks ?? []; if (!tracks.some(t => t.id === this.view.selectedTrack)) this.view.selectedTrack = tracks[0]?.id ?? ''; const clips = new Set(tracks.flatMap(t => t.clips.map(c => c.id))); for (const id of this.view.selectedClips) if (!clips.has(id)) this.view.selectedClips.delete(id); }
}
