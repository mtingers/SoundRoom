import type { DrumPattern } from './generated/models.js';
import { validateModel } from './validate.js';
export interface DrumDocument {
    version: number;
    patterns: DrumPattern[];
    selected: number;
}
export function validateDrumPattern(value: unknown): asserts value is DrumPattern {
    validateModel(value, 'DrumPattern');
    const p = value as DrumPattern, range = (x: number, min: number, max: number) => Number.isFinite(x) && x >= min && x <= max, count = p.bars * 4 * p.stepsPerBeat;
    if (!range(p.bpm, 30, 300) || !Number.isInteger(p.bars) || !range(p.bars, 1, 8) || ![2, 4, 8].includes(p.stepsPerBeat) || !range(p.swing, 0, 0.7) || !range(p.humanize, 0, 0.025) || !range(p.masterGain, 0, 1) || p.name.length > 120 || p.steps.length > 2304 || p.variationCycles != null && ![1, 2, 4, 8].includes(p.variationCycles) || p.velocityHumanize != null && !range(p.velocityHumanize, 0, 0.2))
        throw Error('Unsupported drum pattern values.');
    const seen = new Set<string>();
    for (const s of p.steps) {
        const id = s.voice + ':' + s.index;
        if (seen.has(id) || !Number.isInteger(s.index) || !range(s.index, 0, count - 1) || !range(s.velocity, 0, 1) || !range(s.pan, -1, 1) || !range(s.probability, 0, 1))
            throw Error('Invalid or duplicate drum hit.');
        seen.add(id);
        const available = s.voice === 'snare' ? ['center', 'rimshot', 'sideStick', 'brush'] : ['ride', 'crash'].includes(s.voice) ? ['center', 'bell'] : ['lowTom', 'highTom'].includes(s.voice) ? ['center', 'rimshot'] : ['center'];
        if (s.articulation && !available.includes(s.articulation))
            throw Error('Unsupported drum articulation.');
    }
    for (const l of Object.values(p.lanes))
        if (!range(l.gain, 0, 1.5) || !range(l.pan, -1, 1) || !range(l.tuning, -12, 12) || l.damping != null && !range(l.damping, 0, 1))
            throw Error('Invalid drum lane.');
}
export function remapDrumGrid(pattern: DrumPattern, division: 2 | 4 | 8): DrumPattern { const next = structuredClone(pattern), old = next.stepsPerBeat; next.stepsPerBeat = division; const hits = new Map<string, DrumPattern['steps'][number]>(); for (const source of next.steps) {
    const step = { ...source, index: Math.round(source.index * division / old) };
    if (step.index >= next.bars * 4 * division)
        continue;
    const key = step.voice + ':' + step.index, previous = hits.get(key);
    if (!previous || previous.velocity <= step.velocity)
        hits.set(key, step);
} next.steps = [...hits.values()].sort((a, b) => a.index - b.index); return next; }
export function resizeDrumBars(pattern: DrumPattern, bars: number): DrumPattern { if (!Number.isInteger(bars) || bars < 1 || bars > 8)
    throw Error('Choose 1–8 bars.'); const next = structuredClone(pattern); next.bars = bars; next.steps = next.steps.filter(s => s.index < bars * 4 * next.stepsPerBeat); return next; }
export function parseDrumDocument(text: string): DrumDocument { if (new TextEncoder().encode(text).length > 2000000)
    throw Error('Drum documents are limited to 2 MB.'); const value: unknown = JSON.parse(text); if (!value || typeof value !== 'object' || Array.isArray(value))
    throw Error('Invalid drum document.'); const d = value as DrumDocument; if (Object.keys(d).some(k => !['version', 'patterns', 'selected'].includes(k)) || ![1, 2].includes(d.version) || !Array.isArray(d.patterns) || d.patterns.length < 1 || d.patterns.length > 32 || !Number.isInteger(d.selected) || d.selected < 0 || d.selected >= d.patterns.length)
    throw Error('Invalid drum document.'); d.patterns.forEach(validateDrumPattern); return d; }
export function duplicateDrumPattern(document: DrumDocument, empty = false): DrumDocument { if (document.patterns.length >= 32)
    throw Error('A drum document supports 32 patterns.'); const next = structuredClone(document), pattern = structuredClone(next.patterns[next.selected]!); if (empty) {
    pattern.steps = [];
    pattern.name = 'Pattern ' + (next.patterns.length + 1);
}
else
    pattern.name = pattern.name.slice(0, 115) + ' copy'; next.patterns.push(pattern); next.selected = next.patterns.length - 1; next.version = 2; return next; }
