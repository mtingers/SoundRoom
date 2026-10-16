import {randomUUID} from './crypto.js';
import type { Clip, ClipState, Track, AutomationPoint, RealtimeEffect, PitchEditRegion } from './generated/models.js';
import { automationValue, type Point } from './automation.js';
import { parameterNormalized, type EffectMetadata } from './effects.js';
const uid = () => randomUUID().toUpperCase();
const stateKeys = ['assetID', 'name', 'offset', 'length', 'soundDesign', 'drumPattern', 'renderEnvelope', 'fadeInFrames', 'fadeOutFrames', 'fadeReferenceOffset', 'fadeReferenceLength', 'edgeFadeFrames', 'edgeProtectionDisabled', 'cutFadeInFrames', 'cutFadeOutFrames'] as const;
export function resetEnvelope(clip: Clip): Clip { const result = structuredClone(clip); delete result.renderEnvelope; delete result.fadeReferenceOffset; delete result.fadeReferenceLength; return result; }
/** Editing a boundary changes source extent; it deliberately retires generated-note editing. */
export function trimClip(clip: Clip, assetFrames: number, start?: number, end?: number): Clip {
    let changed = structuredClone(clip);
    if (start !== undefined) {
        changed.start = Math.max(0, clip.start - clip.offset, Math.min(clip.start + clip.length - 1, Math.round(start)));
        const delta = changed.start - clip.start;
        changed.offset += delta;
        changed.length -= delta;
    }
    if (end !== undefined)
        changed.length = Math.max(1, Math.min(assetFrames - changed.offset, Math.round(end) - changed.start));
    if (changed.start === clip.start && changed.length === clip.length)
        return changed;
    delete changed.soundDesign;
    delete changed.drumPattern;
    return resetEnvelope(changed);
}
/** Preserve source envelope coordinates when copying only part of an existing clip. */
export function fragment(clip: Clip, lo: number, hi: number, newID = true): Clip | undefined {
    const start = Math.max(clip.start, lo), end = Math.min(clip.start + clip.length, hi);
    if (end <= start)
        return undefined;
    const result = structuredClone(clip), delta = start - clip.start;
    result.id = newID ? uid() : clip.id;
    result.start = start;
    result.offset += delta;
    result.length = end - start;
    if (delta || result.length !== clip.length) {
        if (result.renderEnvelope) {
            const envelope = result.renderEnvelope;
            envelope.referenceOffset = (envelope.referenceOffset ?? 0) + delta;
        }
        if ((clip.fadeInFrames ?? 0) > 0 || (clip.fadeOutFrames ?? 0) > 0 || (clip.edgeFadeFrames ?? 0) > 0) {
            result.fadeReferenceLength = clip.fadeReferenceLength ?? clip.length;
            result.fadeReferenceOffset = (clip.fadeReferenceOffset ?? 0) + delta;
        }
        if (delta)
            result.cutFadeInFrames = clip.edgeFadeFrames;
        if (end !== clip.start + clip.length)
            result.cutFadeOutFrames = clip.edgeFadeFrames;
        delete result.soundDesign;
        delete result.drumPattern;
    }
    return result;
}
export function deleteRange(track: Track, start: number, end: number): Track {
    const result = structuredClone(track);
    result.clips = (track.clips ?? []).flatMap(clip => {
        if (clip.start >= end || clip.start + clip.length <= start)
            return [structuredClone(clip)];
        return [fragment(clip, clip.start, start, false), fragment(clip, end, clip.start + clip.length, true)].filter((c): c is Clip => !!c);
    });
    return result;
}
export function clipFromHistory(clip: Clip, state: ClipState, newTrack = false): Clip {
    const result = structuredClone(clip);
    for (const key of stateKeys) {
        delete (result as unknown as Record<string, unknown>)[key];
        const value = state[key];
        if (value !== undefined && value !== null)
            (result as unknown as Record<string, unknown>)[key] = structuredClone(value);
    }
    if (newTrack) {
        result.id = uid();
        delete result.previousStates;
    }
    return result;
}
export function newTrack(name: string, color = 0): Track { return { id: uid(), name, color, clips: [], recordingTakes: [], gain: 1, pan: 0, muted: false, solo: false, armed: false, inputChannel: 0, stereoInput: false, realtimeEffects: [], mixAutomation: {} }; }
export interface AudioClipboard {
    kind: 'audio';
    serverID: string;
    clips: Clip[];
}
export interface TrackClipboard {
    kind: 'track';
    serverID: string;
    track: Track;
    automation: boolean;
}
export type Clipboard = AudioClipboard | TrackClipboard;
export function copyAudio(track: Track, selection: Set<string>, range?: [
    number,
    number
]): Clip[] {
    const selected = (track.clips ?? []).filter(c => selection.has(c.id));
    let clips: Clip[];
    if (selection.size > 1 || !range)
        clips = selected.map(c => structuredClone(c));
    else
        clips = (track.clips ?? []).map(c => fragment(c, range[0], range[1])).filter((c): c is Clip => !!c);
    const origin = range && selection.size <= 1 ? range[0] : Math.min(...clips.map(c => c.start));
    return clips.map(c => ({ ...c, start: c.start - origin }));
}
export function copyTrack(source: Track, automation: boolean): Track {
    const result = structuredClone(source);
    if (!automation) {
        result.mixAutomation = {};
        for (const effect of result.realtimeEffects ?? []) {
            effect.automation = [];
            effect.parameterAutomation = {};
            effect.pitchEdits = [];
            effect.pitchEditsOnly = false;
        }
    }
    return result;
}
export function insertBoundary(points: AutomationPoint[], frame: number, baseline: number, decibels = false): AutomationPoint[] {
    const sorted = points.map(p => ({ ...p })).sort((a, b) => a.frame - b.frame);
    if (sorted.some(p => p.frame === frame))
        return sorted;
    const value = automationValue(sorted as Point[], frame, baseline, decibels);
    const lower = sorted.map(p => p.frame < frame).lastIndexOf(true);
    if (lower < 0 || lower + 1 >= sorted.length)
        return [...sorted, { frame, value, curve: 'linear' as const, curveStart: 0, curveEnd: 1 }].sort((a, b) => a.frame - b.frame);
    const original = sorted[lower]!, next = sorted[lower + 1]!, start = original.curveStart ?? 0, end = original.curveEnd ?? 1, middle = start + (end - start) * (frame - original.frame) / (next.frame - original.frame);
    sorted[lower]!.curveEnd = middle;
    sorted.splice(lower + 1, 0, { frame, value, curve: original.curve ?? 'linear', curveStart: middle, curveEnd: end });
    return sorted;
}
function shiftedCurve(points: AutomationPoint[], shift: number, baseline: number, decibels = false): AutomationPoint[] {
    const retained = shift < 0 ? insertBoundary(points, -shift, baseline, decibels) : points;
    const result = retained.filter(p => p.frame + shift >= 0).map(p => ({ ...p, frame: p.frame + shift }));
    if (points.some(p => p.frame + shift < 0) && !result.some(p => p.frame === 0))
        result.push({ frame: 0, value: automationValue(points as Point[], -shift, baseline, decibels), curve: points.filter(p => p.frame <= -shift).at(-1)?.curve ?? 'linear', curveStart: 0, curveEnd: 1 });
    return result.sort((a, b) => a.frame - b.frame);
}
function shiftedRegions(regions: PitchEditRegion[], shift: number): PitchEditRegion[] { return regions.filter(r => r.endFrame + shift > 0).map(r => { const original = r.points ?? [], curve = original.map(p => ({ ...p, value: p.note })), points = shift < 0 && r.startFrame + shift < 0 ? insertBoundary(curve, -shift, original[0]?.note ?? 60) : curve; return { ...r, id: uid(), startFrame: Math.max(0, r.startFrame + shift), endFrame: r.endFrame + shift, points: points.filter(p => p.frame + shift >= 0).map(p => ({ id: uid(), frame: p.frame + shift, note: p.value, curve: p.curve, curveStart: p.curveStart, curveEnd: p.curveEnd })) }; }); }
/** New identities prevent copied controls and takes from aliasing the original track. */
export function pasteTrack(source: Track, destination: number, names: Set<string>, maximumFrame: number, metadata: EffectMetadata[] = []): Track {
    const origin = Math.min(...(source.clips ?? []).map(c => c.start), ...(source.clips?.length ? [] : [0])), shift = destination - origin, result = structuredClone(source);
    result.id = uid();
    result.armed = false;
    const base = `${source.name ?? 'Track'} copy`;
    result.name = base;
    for (let suffix = 2; names.has(result.name); suffix++)
        result.name = `${base} ${suffix}`;
    const relocate = (clip: Clip): Clip | undefined => { const moved = { ...structuredClone(clip), id: uid(), start: clip.start + shift }; if (moved.start + moved.length > maximumFrame)
        throw new Error('The pasted track exceeds the timeline limit.'); if (moved.start < 0) {
        const part = fragment(moved, 0, moved.start + moved.length, false);
        return part;
    } return moved; };
    result.clips = (source.clips ?? []).map(relocate).filter((c): c is Clip => !!c);
    const active = (source.recordingTakes ?? []).findIndex(t => t.id === source.activeTakeID);
    result.recordingTakes = (source.recordingTakes ?? []).map(t => ({ ...structuredClone(t), id: uid(), clips: t.clips.map(relocate).filter((c): c is Clip => !!c) }));
    result.activeTakeID = active >= 0 ? result.recordingTakes[active]?.id : undefined;
    result.mixAutomation = {};
    for (const [parameter, points] of Object.entries(source.mixAutomation ?? {}))
        result.mixAutomation[parameter] = shiftedCurve(points, shift, parameter === 'volume' ? (source.gain ?? 1) / 1.5 : ((source.pan ?? 0) + 1) / 2, parameter === 'volume' && source.volumeAutomationScale === 'decibels');
    result.realtimeEffects = (source.realtimeEffects ?? []).map(effect => { const updated: RealtimeEffect = { ...structuredClone(effect), id: uid(), automation: shiftedCurve(effect.automation ?? [], shift, effect.amount ?? 0.5), parameterAutomation: {}, pitchEdits: shiftedRegions(effect.pitchEdits ?? [], shift), bakedRanges: (effect.bakedRanges ?? []).filter(r => r.endFrame + shift > 0).map(r => ({ startFrame: Math.max(0, r.startFrame + shift), endFrame: r.endFrame + shift })) }; for (const [key, points] of Object.entries(effect.parameterAutomation ?? {})) {
        const descriptor = metadata.find(m => m.id === effect.kind)?.parameters.find(p => p.id === key), baseline = descriptor ? parameterNormalized(effect.parameters?.[key] ?? descriptor.defaultValue, descriptor) : 0.5;
        updated.parameterAutomation![key] = shiftedCurve(points, shift, baseline);
    } return updated; });
    for (const curve of [...Object.values(result.mixAutomation), ...(result.realtimeEffects ?? []).flatMap(e => [e.automation ?? [], ...Object.values(e.parameterAutomation ?? {})])])
        if (curve.some(p => p.frame > maximumFrame))
            throw new Error('Pasted automation exceeds the timeline limit.');
    return result;
}
