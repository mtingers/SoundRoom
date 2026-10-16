import type { Clip } from './model.js';
/** Immutable metadata arrays are indexed once; transport frames do no full-track scans. */
export class ClipIndex {
    readonly clips: Clip[];
    private ends: number[];
    private positions = new Map<string, number>();
    constructor(source: Clip[]) { this.clips = [...source].sort((a, b) => a.start - b.start || a.id.localeCompare(b.id)); this.clips.forEach((c, i) => this.positions.set(c.id, i)); let maximum = 0; this.ends = this.clips.map(c => maximum = Math.max(maximum, c.start + c.length)); }
    visible(start: number, end: number): Clip[] { let lo = 0, hi = this.clips.length; while (lo < hi) {
        const middle = (lo + hi) >>> 1;
        if (this.clips[middle]!.start < end)
            lo = middle + 1;
        else
            hi = middle;
    } const last = lo; lo = 0; hi = last; while (lo < hi) {
        const middle = (lo + hi) >>> 1;
        if (this.ends[middle]! <= start)
            lo = middle + 1;
        else
            hi = middle;
    } const result: Clip[] = []; for (let i = lo; i < last; i++) {
        const clip = this.clips[i]!;
        if (clip.start + clip.length > start)
            result.push(clip);
    } return result; }
    at(frame: number) { return this.visible(frame, frame + 1)[0]; }
    overlaps(start: number, end: number) { return this.visible(start, end).flatMap(clip => { const index = this.positions.get(clip.id)!, next = this.clips[index + 1]; return next && next.start > clip.start && next.start < clip.start + clip.length && next.start + next.length >= clip.start + clip.length && clip.edgeProtectionDisabled !== true && next.edgeProtectionDisabled !== true && (clip.edgeFadeFrames ?? 0) > 0 && (next.edgeFadeFrames ?? 0) > 0 && !clip.renderEnvelope && !next.renderEnvelope ? [{ outgoing: clip, incoming: next, start: next.start, end: clip.start + clip.length }] : []; }); }
}
