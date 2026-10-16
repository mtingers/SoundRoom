import type { Parameter } from './effects.js';
export function parameterDisplay(value: number, p: Parameter) { if (p.choices)
    return p.choices[Math.round(value - p.minimum)] ?? String(value); if (p.unit === '%')
    return `${(value * 100).toFixed(0)}%`; if (p.unit === 'Hz' && value >= 1000)
    return `${(value / 1000).toFixed(2)} kHz`; return `${Number(value.toFixed(p.integer ? 0 : 2))}${p.unit ? ' ' + p.unit : ''}`; }
export function inputScale(p: Parameter) { return p.unit === '%' ? 100 : 1; }
export const effectGroups: Record<string, {
    name: string;
    keys: string[];
}[]> = { gain: [{ name: 'Level', keys: ['gain'] }], lowPass: [{ name: 'Filter', keys: ['frequency', 'resonance'] }], highPass: [{ name: 'Filter', keys: ['frequency', 'resonance'] }], echo: [{ name: 'Repeats', keys: ['time', 'feedback'] }, { name: 'Tone & balance', keys: ['damping', 'mix'] }], warmth: [{ name: 'Character', keys: ['character', 'drive', 'bias', 'emphasis', 'tone'] }, { name: 'Output', keys: ['match', 'mix', 'output'] }], compressor: [{ name: 'Compression', keys: ['threshold', 'ratio', 'knee'] }, { name: 'Timing', keys: ['attack', 'release'] }, { name: 'Output', keys: ['makeup', 'mix'] }], room: [{ name: 'Space', keys: ['character', 'decay', 'predelay'] }, { name: 'Dense stereo · reflections', keys: ['diffusion', 'early', 'width'] }, { name: 'Tone', keys: ['lowCut', 'damping'] }, { name: 'Output', keys: ['mix', 'output'] }], equalizer: [{ name: 'Low band', keys: ['lowShape', 'lowFrequency', 'lowGain', 'lowQ'] }, { name: 'Mid bell', keys: ['midFrequency', 'midGain', 'midQ'] }, { name: 'High band', keys: ['highShape', 'highFrequency', 'highGain', 'highQ'] }, { name: 'Cut filters', keys: ['highPassEnabled', 'highPassFrequency', 'lowPassEnabled', 'lowPassFrequency'] }, { name: 'Output', keys: ['output'] }], chorus: [{ name: 'Movement', keys: ['rate', 'depth', 'delay'] }, { name: 'Stereo & repeats', keys: ['width', 'feedback'] }, { name: 'Balance', keys: ['mix'] }], limiter: [{ name: 'Limiting', keys: ['input', 'ceiling'] }, { name: 'Recovery', keys: ['style', 'release', 'truePeak'] }], resonance: [{ name: 'Detection', keys: ['depth', 'sensitivity', 'sharpness'] }, { name: 'Frequency region', keys: ['low', 'high'] }, { name: 'Timing', keys: ['attack', 'release', 'transients'] }, { name: 'Listen & balance', keys: ['delta', 'mix'] }], dither: [{ name: 'Resolution', keys: ['bits', 'quantize'] }, { name: 'Dither', keys: ['amount'] }], pitchCorrection: [{ name: 'Targets', keys: ['scale', 'key', 'mask', 'manualTarget'] }, { name: 'Correction', keys: ['retune', 'strength', 'humanize', 'vibrato'] }, { name: 'Voice', keys: ['formant', 'mix', 'reference', 'tolerance'] }] };
export const effectIdentity: Record<string, {
    icon: string;
    color: string;
}> = { gain: { icon: '↕', color: '#93b5cc' }, lowPass: { icon: '◣', color: '#7bcab9' }, highPass: { icon: '◢', color: '#7bcab9' }, equalizer: { icon: '∿', color: '#7bcab9' }, compressor: { icon: '↧', color: '#dfba78' }, limiter: { icon: '⊓', color: '#dfba78' }, resonance: { icon: '⋮', color: '#8db8de' }, echo: { icon: '⋯', color: '#bca4e2' }, room: { icon: '◇', color: '#bca4e2' }, chorus: { icon: '≈', color: '#bca4e2' }, warmth: { icon: '◒', color: '#e6a081' }, dither: { icon: '▧', color: '#a6b3bf' }, pitchCorrection: { icon: '♩', color: '#d6a2c2' } };
export function compressorOutput(input: number, p: Record<string, number>) { const over = input - p.threshold!, slope = 1 / Math.max(1, p.ratio!) - 1, knee = p.knee ?? 0, reduction = knee > 0 && Math.abs(over) < knee / 2 ? slope * (over + knee / 2) ** 2 / (2 * knee) : over > 0 ? slope * over : 0; return input + 20 * Math.log10(Math.max(1e-15, 1 - (p.mix ?? 1) + (p.mix ?? 1) * 10 ** ((reduction + (p.makeup ?? 0)) / 20))); }
export function warmthCurve(input: number, p: Record<string, number>) { const drive = 10 ** ((p.drive ?? 0) / 20), x = input * drive, bias = p.bias ?? 0.25; switch (Math.round(p.character ?? 0)) {
    case 1: return Math.atan(x * 1.4) / Math.atan(drive * 1.4);
    case 2: {
        const offset = bias * 0.8;
        return (Math.tanh(x + offset) - Math.tanh(offset)) / Math.max(0.1, (Math.tanh(drive + offset) - Math.tanh(-drive + offset)) * 0.5);
    }
    case 3: return x / Math.sqrt(1 + x * x) * Math.sqrt(1 + drive * drive) / drive;
    case 4: return Math.tanh(x * (x >= 0 ? 1 + bias : 1 - bias * 0.45) * 1.8) / Math.max(0.1, Math.tanh(drive * 1.8));
    default: return Math.tanh(x) / Math.max(0.00001, Math.tanh(drive));
} }
type Shape = 'bell' | 'lowShelf' | 'highShelf' | 'highPass' | 'lowPass';
export function sectionResponse(frequency: number, q: number, gain: number, shape: Shape, probe: number, rate: number): number { const omega = 2 * Math.PI * Math.min(rate * 0.45, Math.max(20, frequency)) / rate, c = Math.cos(omega), alpha = Math.sin(omega) / (2 * Math.max(0.001, q)), a = 10 ** (gain / 40), beta = 2 * Math.sqrt(a) * alpha; let b0 = 0, b1 = 0, b2 = 0, a0 = 0, a1 = 0, a2 = 0; switch (shape) {
    case 'bell':
        a0 = 1 + alpha / a;
        b0 = 1 + alpha * a;
        b1 = -2 * c;
        b2 = 1 - alpha * a;
        a1 = -2 * c;
        a2 = 1 - alpha / a;
        break;
    case 'lowShelf':
        a0 = a + 1 + (a - 1) * c + beta;
        b0 = a * (a + 1 - (a - 1) * c + beta);
        b1 = 2 * a * (a - 1 - (a + 1) * c);
        b2 = a * (a + 1 - (a - 1) * c - beta);
        a1 = -2 * (a - 1 + (a + 1) * c);
        a2 = a + 1 + (a - 1) * c - beta;
        break;
    case 'highShelf':
        a0 = a + 1 - (a - 1) * c + beta;
        b0 = a * (a + 1 + (a - 1) * c + beta);
        b1 = -2 * a * (a - 1 + (a + 1) * c);
        b2 = a * (a + 1 + (a - 1) * c - beta);
        a1 = 2 * (a - 1 - (a + 1) * c);
        a2 = a + 1 - (a - 1) * c - beta;
        break;
    case 'highPass':
        a0 = 1 + alpha;
        b0 = (1 + c) / 2;
        b1 = -(1 + c);
        b2 = b0;
        a1 = -2 * c;
        a2 = 1 - alpha;
        break;
    case 'lowPass':
        a0 = 1 + alpha;
        b0 = (1 - c) / 2;
        b1 = 1 - c;
        b2 = b0;
        a1 = -2 * c;
        a2 = 1 - alpha;
        break;
} const probeOmega = 2 * Math.PI * Math.min(rate / 2, Math.max(0, probe)) / rate, cc = Math.cos(probeOmega), ss = Math.sin(probeOmega), c2 = Math.cos(2 * probeOmega), s2 = Math.sin(2 * probeOmega), nr = (b0 + b1 * cc + b2 * c2) / a0, ni = (-b1 * ss - b2 * s2) / a0, dr = 1 + a1 / a0 * cc + a2 / a0 * c2, di = -a1 / a0 * ss - a2 / a0 * s2; return Math.sqrt((nr * nr + ni * ni) / Math.max(1e-30, dr * dr + di * di)); }
export function eqResponse(probe: number, p: Record<string, number>, rate = 48000) { let value = sectionResponse(p.lowFrequency ?? 100, p.lowQ ?? 0.707, p.lowGain ?? 0, (p.lowShape ?? 0) >= 0.5 ? 'lowShelf' : 'bell', probe, rate) * sectionResponse(p.midFrequency ?? 1000, p.midQ ?? 1, p.midGain ?? 0, 'bell', probe, rate) * sectionResponse(p.highFrequency ?? 8000, p.highQ ?? 0.707, p.highGain ?? 0, (p.highShape ?? 0) >= 0.5 ? 'highShelf' : 'bell', probe, rate) * 10 ** ((p.output ?? 0) / 20); if ((p.highPassEnabled ?? 0) >= 0.5)
    value *= sectionResponse(p.highPassFrequency ?? 60, 1 / Math.sqrt(2), 0, 'highPass', probe, rate); if ((p.lowPassEnabled ?? 0) >= 0.5)
    value *= sectionResponse(p.lowPassFrequency ?? 16000, 1 / Math.sqrt(2), 0, 'lowPass', probe, rate); return 20 * Math.log10(Math.max(1e-15, value)); }
import type { RealtimeEffect } from './generated/models.js';
import { parameterNormalized, type EffectMetadata } from './effects.js';
/** Semantic equality for a verified no-op: native parameter/curve scalars are Float, frames remain exact. */
export function sameEffectSettings(a: RealtimeEffect | undefined, b: RealtimeEffect): boolean {
    if (!a) return false;
    const curve = <T extends {curve?:string;curveStart?:number;curveEnd?:number}>(p:T) => ({...p,curve:p.curve??'linear',curveStart:p.curveStart??0,curveEnd:p.curveEnd??1});
    const defaults = (e: RealtimeEffect) => ({ ...e, amount: e.amount ?? 0.5, enabled: e.enabled ?? true, parameters: e.parameters ?? {}, automation: (e.automation ?? []).map(curve), parameterAutomation: Object.fromEntries(Object.entries(e.parameterAutomation??{}).map(([key,points])=>[key,points.map(curve)])), pitchEdits: (e.pitchEdits??[]).map(r=>({...r,points:(r.points??[]).map(curve)})), pitchEditsOnly: e.pitchEditsOnly ?? false, bakedRanges: e.bakedRanges ?? [] });
    const canonical = (value: unknown, key = ''): unknown => {
        if (typeof value === 'number') return ['frame','startFrame','endFrame'].includes(key) ? value : Math.fround(value);
        if (typeof value === 'string') return key === 'id' ? value.toUpperCase() : value;
        if (Array.isArray(value)) return value.map(v => canonical(v));
        if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).filter(([,v])=>v!=null).sort(([a],[b])=>a.localeCompare(b)).map(([k,v])=>[k,canonical(v,k)]));
        return value;
    };
    return JSON.stringify(canonical(defaults(a))) === JSON.stringify(canonical(defaults(b)));
}
export function editableEffect(source: RealtimeEffect, meta: EffectMetadata): RealtimeEffect { const effect = structuredClone(source); if (Object.keys(effect.parameters ?? {}).length)
    return effect; const amount = Math.max(0, Math.min(1, effect.amount ?? 0.5)), values = Object.fromEntries(meta.parameters.map(p => [p.id, p.defaultValue])); const physical = (a: number) => { switch (effect.kind) {
    case 'gain': return a * 48 - 24;
    case 'lowPass': return 200 * 80 ** (1 - a);
    case 'echo': return a * 0.65;
    case 'warmth': return 20 * Math.log10(1 + a * 8);
    default: return 0;
} }; const primary = ({ gain: 'gain', lowPass: 'frequency', echo: 'mix', warmth: 'drive' } as Record<string, string>)[effect.kind]; if (primary)
    values[primary] = physical(amount); if (effect.kind === 'echo') {
    values.time = 250;
    values.feedback = 0.4;
    values.damping = 20000;
} if (effect.kind === 'warmth')
    values.tone = 20000; if (effect.kind === 'room')
    values.character = 0; effect.parameters = values; effect.parameterAutomation ??= {}; const descriptor = meta.parameters.find(p => p.id === primary); if (descriptor && !effect.parameterAutomation[primary!]?.length)
    effect.parameterAutomation[primary!] = (effect.automation ?? []).map(point => ({ ...point, value: parameterNormalized(physical(point.value), descriptor) })); return effect; }
