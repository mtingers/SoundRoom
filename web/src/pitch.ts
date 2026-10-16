import {randomUUID} from './crypto.js';
import type { PitchEditRegion, PitchEditPoint, AutomationCurve } from './generated/models.js';
import { insertBoundary } from './arranging.js';
export function frequencyNote(frequency: number, reference = 440) { return 69 + 12 * Math.log2(frequency / reference); }
export function sliceRegion(region: PitchEditRegion, start: number, end: number): PitchEditRegion | undefined {
    start = Math.max(start, region.startFrame);
    end = Math.min(end, region.endFrame);
    if (end <= start || !region.points?.length)
        return undefined;
    if (start === region.startFrame && end === region.endFrame)
        return structuredClone(region);
    const points = region.points.map(p => ({ ...p, value: p.note })), baseline = points[0]!.note, split = insertBoundary(insertBoundary(points, start, baseline), end, baseline);
    return { ...region, id: randomUUID().toUpperCase(), startFrame: start, endFrame: end, points: split.filter(p => p.frame >= start && p.frame <= end).map(p => ({ id: region.points!.find(original => original.frame === p.frame)?.id ?? randomUUID().toUpperCase(), frame: p.frame, note: p.value, curve: p.curve, curveStart: p.curveStart, curveEnd: p.curveEnd })) };
}
/** Scope changes cannot erase pitch targets outside the reviewed selection. */
export function replacePitchScope(existing: PitchEditRegion[], updates: PitchEditRegion[], scope: [
    number,
    number
]): PitchEditRegion[] {
    const retained = existing.flatMap(r => r.endFrame <= scope[0] || r.startFrame >= scope[1] ? [structuredClone(r)] : [sliceRegion(r, r.startFrame, scope[0]), sliceRegion(r, scope[1], r.endFrame)].filter((r): r is PitchEditRegion => !!r));
    return [...retained, ...updates.map(r => sliceRegion(r, scope[0], scope[1])).filter((r): r is PitchEditRegion => !!r)].sort((a, b) => a.startFrame - b.startFrame);
}
export function editPitchPoint(regions: PitchEditRegion[], id: string, frame: number, note: number, curve?: AutomationCurve): PitchEditRegion[] { return regions.map(r => { const point = r.points?.find(p => p.id === id); if (!point)
    return structuredClone(r); const moved = { ...point, frame: Math.max(r.startFrame, Math.min(r.endFrame, Math.round(frame))), note: Math.max(0, Math.min(95, note)), ...(curve ? { curve, curveStart: 0, curveEnd: 1 } : {}) }; return { ...r, points: [...(r.points ?? []).filter(p => p.id !== id && p.frame !== moved.frame), moved].sort((a, b) => a.frame - b.frame) }; }); }
export function pitchPointCount(regions: PitchEditRegion[]) { return regions.reduce((n, r) => n + (r.points?.length ?? 0), 0); }
export function addPitchPoint(regions: PitchEditRegion[], frame: number, note: number): {
    regions: PitchEditRegion[];
    point: PitchEditPoint;
} | undefined { const region = regions.find(r => frame >= r.startFrame && frame < r.endFrame); if (!region)
    return undefined; const point: PitchEditPoint = { id: randomUUID().toUpperCase(), frame: Math.round(frame), note: Math.max(0, Math.min(95, note)), curve: 'smooth', curveStart: 0, curveEnd: 1 }; return { regions: regions.map(r => r === region ? { ...r, points: [...(r.points ?? []).filter(p => p.frame !== point.frame), point].sort((a, b) => a.frame - b.frame) } : r), point }; }
