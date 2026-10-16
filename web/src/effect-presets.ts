import {randomUUID} from './crypto.js';
import {parseStrictJSON as parseJSON} from './strict-json.js';
import type { RealtimeEffect } from './generated/models.js';
import type { EffectMetadata } from './effects.js';
import { validateModel } from './validate.js';

export interface SavedEffectPreset { id: string; name: string; kind: RealtimeEffect['kind']; parameters: Record<string, number>; legacyAmount?: number }
export interface SavedEffectChain { id: string; name: string; effects: RealtimeEffect[]; includesAutomation: boolean }
const MAX_PRESETS = 1_048_576, MAX_CHAINS = 4_194_304, MAX_LIBRARY = 4_194_304;
const uuid = () => randomUUID().toUpperCase();
const bytes = (text: string) => new TextEncoder().encode(text).length;
let segmenter: Intl.Segmenter | undefined;
function nameGraphemes(name: string) {
    if (typeof Intl.Segmenter !== 'function') throw Error('Native preset names require a browser with Intl.Segmenter support.');
    segmenter ??= new Intl.Segmenter('en', { granularity: 'grapheme' });
    return segmenter.segment(name);
}
function namePrefix(name: string, count: number) { let prefix = '', n = 0; for (const p of nameGraphemes(name)) { if (n++ >= count) break; prefix += p.segment; } return prefix; }
function record(value: unknown, fields: string[], required = fields): Record<string, unknown> {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw Error('Invalid preset document.');
    const object = value as Record<string, unknown>;
    if (Object.keys(object).some(k => !fields.includes(k)) || required.some(k => !Object.hasOwn(object, k))) throw Error('Unknown or missing preset field.');
    return object;
}
function id(value: unknown): string { validateModel(value, 'UUID'); return (value as string).toUpperCase(); }
export function presetName(value: unknown): string {
    if (typeof value !== 'string') throw Error('Preset names must be text.');
    const name = value.trim();
    let count = 0; for (const _ of nameGraphemes(name)) { if (++count > 80) break; }
    if (!name || count > 80 || /[\p{Cc}\p{Cf}]/u.test(name)) throw Error('Use a preset name of 1–80 characters on one line.');
    return name;
}
/** Foundation's POSIX case/diacritic folding expands sharp-s/ligatures and equates sigma forms.
 * Per-code-point casing avoids contextual final sigma; dotless i remains distinct, as on the Mac. */
const nameKey = (name: string) => {
    let result = '', romanGreekCyrillic = false;
    for (const c of name.normalize('NFD')) {
        if (/\p{M}/u.test(c)) { if (romanGreekCyrillic) continue; }
        else romanGreekCyrillic = c.codePointAt(0)! < 0x0510;
        result += c === 'ı' ? c : c.toUpperCase().toLowerCase().replace(/ß/g, 'ss');
    }
    return result;
};
function parameters(value: unknown, metadata: EffectMetadata): Record<string, number> {
    const values = record(value, metadata.parameters.map(p => p.id), []);
    if (Object.keys(values).length > 64) throw Error('Too many effect parameters.');
    for (const [key, value] of Object.entries(values)) {
        const p = metadata.parameters.find(p => p.id === key)!;
        if (typeof value !== 'number' || !Number.isFinite(value) || value < p.minimum || value > p.maximum || ((p.integer || p.choices) && !Number.isInteger(value))) throw Error(`${p.name} must be within ${p.minimum}–${p.maximum} ${p.unit}.`);
    }
    return { ...values } as Record<string, number>;
}
const metadataFor = (kind: unknown, metadata: EffectMetadata[]) => metadata.find(m => m.id === kind) ?? (() => { throw Error('This effect is not advertised by the hardware server.'); })();
function checkedPreset(value: unknown, metadata: EffectMetadata[]): SavedEffectPreset {
    const p = record(value, ['id', 'name', 'kind', 'parameters', 'legacyAmount'], ['id', 'name', 'kind', 'parameters']);
    validateModel(p.kind, 'RealtimeEffectKind');
    const m = metadataFor(p.kind, metadata);
    if (p.legacyAmount != null && (typeof p.legacyAmount !== 'number' || !Number.isFinite(p.legacyAmount) || p.legacyAmount < 0 || p.legacyAmount > 1)) throw Error('Legacy amount must be within 0–1.');
    return { id: id(p.id), name: presetName(p.name), kind: m.id as RealtimeEffect['kind'], parameters: parameters(p.parameters, m), ...(p.legacyAmount != null ? { legacyAmount: p.legacyAmount as number } : {}) };
}
function points(value: RealtimeEffect['automation']) {
    if ((value?.length ?? 0) > 4096) throw Error('Automation lanes support at most 4096 points.');
    const frames = new Set<number>();
    for (const p of value ?? []) {
        if (!Number.isSafeInteger(p.frame) || p.frame < 0 || p.value < 0 || p.value > 1 || frames.has(p.frame) || (p.curveStart ?? 0) < 0 || (p.curveEnd ?? 1) > 1 || (p.curveStart ?? 0) > (p.curveEnd ?? 1)) throw Error('Invalid or duplicate automation point.');
        frames.add(p.frame);
    }
}
export function checkedEffect(value: unknown, metadata: EffectMetadata[]): RealtimeEffect {
    validateModel(value, 'RealtimeEffect');
    const effect = structuredClone(value as RealtimeEffect), m = metadataFor(effect.kind, metadata);
    if (effect.id) effect.id = id(effect.id);
    if (effect.amount != null && (effect.amount < 0 || effect.amount > 1)) throw Error('Effect amount must be within 0–1.');
    effect.parameters = parameters(effect.parameters ?? {}, m);
    points(effect.automation);
    for (const [key, lane] of Object.entries(effect.parameterAutomation ?? {})) { if (!m.parameters.some(p => p.id === key)) throw Error('Unknown automated effect parameter.'); points(lane); }
    if ((effect.pitchEdits?.length ?? 0) > 4096 || (effect.bakedRanges?.length ?? 0) > 4096) throw Error('Too many pitch regions.');
    const regionIDs = new Set<string>();
    for (const r of effect.pitchEdits ?? []) {
        if (r.startFrame < 0 || r.endFrame <= r.startFrame || (r.points?.length ?? 0) > 4096 || r.id && regionIDs.has(id(r.id))) throw Error('Invalid pitch region.');
        if (r.id) regionIDs.add(id(r.id));
        const frames = new Set<number>();
        for (const p of r.points ?? []) { if (p.frame < r.startFrame || p.frame > r.endFrame || p.note < 0 || p.note > 95 || frames.has(p.frame)) throw Error('Invalid pitch target.'); frames.add(p.frame); }
    }
    for (const r of effect.bakedRanges ?? []) if (r.startFrame < 0 || r.endFrame <= r.startFrame) throw Error('Invalid rendered audio guard.');
    return effect;
}
function unique<T extends { id: string; name: string }>(values: T[], key: (p: T) => string) {
    const ids = new Set<string>(), names = new Set<string>();
    for (const p of values) { if (ids.has(p.id.toUpperCase()) || names.has(key(p))) throw Error('Preset IDs and names must be unique.'); ids.add(p.id.toUpperCase()); names.add(key(p)); }
    return values;
}
export function parseEffectPresets(text: string, metadata: EffectMetadata[]): SavedEffectPreset[] {
    if (bytes(text) > MAX_PRESETS) throw Error('Effect preset files are limited to 1 MiB.');
    const document = record(parseJSON(text), ['version', 'presets']);
    if (document.version !== 1) throw Error('Unsupported effect preset version.');
    if (!Array.isArray(document.presets) || document.presets.length < 1 || document.presets.length > 128) throw Error('Effect preset files support 1–128 entries.');
    return unique(document.presets.map(p => checkedPreset(p, metadata)), p => `${p.kind}:${nameKey(p.name)}`);
}
export function encodeEffectPresets(presets: SavedEffectPreset[], metadata: EffectMetadata[]): string {
    const text = JSON.stringify({ version: 1, presets }, null, 2); parseEffectPresets(text, metadata); return text;
}
function checkedChain(value: unknown, metadata: EffectMetadata[]): SavedEffectChain {
    const c = record(value, ['id', 'name', 'effects', 'includesAutomation']);
    if (!Array.isArray(c.effects) || c.effects.length > 64 || typeof c.includesAutomation !== 'boolean') throw Error('Chains support up to 64 effects and an explicit automation choice.');
    const effects = c.effects.map(e => checkedEffect(e, metadata));
    const ids = effects.filter(e => e.id).map(e => e.id!.toUpperCase());
    if (new Set(ids).size !== ids.length) throw Error('Duplicate insert identity in chain.');
    return { id: id(c.id), name: presetName(c.name), effects, includesAutomation: c.includesAutomation };
}
export function parseEffectChains(text: string, metadata: EffectMetadata[]): SavedEffectChain[] {
    if (bytes(text) > MAX_CHAINS) throw Error('Chain files are limited to 4 MiB.');
    const document = record(parseJSON(text), ['version', 'chains']);
    if (document.version !== 1 && document.version !== 2) throw Error('Unsupported effect chain version.');
    if (!Array.isArray(document.chains) || document.chains.length < 1 || document.chains.length > 128) throw Error('Chain files support 1–128 entries.');
    return unique(document.chains.map(c => checkedChain(c, metadata)), c => nameKey(c.name));
}
export function encodeEffectChains(chains: SavedEffectChain[], metadata: EffectMetadata[]): string {
    const advanced = chains.some(c => c.effects.some(e => e.pitchEdits?.length || e.pitchEditsOnly || e.bakedRanges?.length));
    const text = JSON.stringify({ version: advanced ? 2 : 1, chains }, null, 2); parseEffectChains(text, metadata); return text;
}
export function presetFromEffect(name: string, effect: RealtimeEffect, metadata: EffectMetadata[]): SavedEffectPreset {
    const m = metadataFor(effect.kind, metadata), modern = Object.keys(effect.parameters ?? {}).length > 0;
    return checkedPreset({ id: uuid(), name, kind: effect.kind, parameters: modern ? Object.fromEntries(m.parameters.map(p => [p.id, effect.parameters?.[p.id] ?? p.defaultValue])) : {}, ...(!modern ? { legacyAmount: effect.amount ?? 0.5 } : {}) }, metadata);
}
/** A parameter preset preserves the target media's rendered guards, manual pitch edits and automation. */
export function applyEffectPreset(effect: RealtimeEffect, preset: SavedEffectPreset, metadata: EffectMetadata[]): RealtimeEffect {
    const p = checkedPreset(preset, metadata); if (effect.kind !== p.kind) throw Error('This preset belongs to a different effect.');
    const value = structuredClone(effect), m = metadataFor(p.kind, metadata);
    if (p.legacyAmount != null) { value.amount = p.legacyAmount; value.parameters = {}; value.selectedAutomationParameter = null; }
    else { value.parameters = Object.fromEntries(m.parameters.map(p => [p.id, preset.parameters[p.id] ?? p.defaultValue])); value.selectedAutomationParameter ??= m.parameters[0]?.id; }
    return value;
}
/** Chain settings target different media: never copy baked guards that would bypass processing there. */
export function prepareEffectChain(chain: SavedEffectChain, metadata: EffectMetadata[]): RealtimeEffect[] {
    return checkedChain(chain, metadata).effects.map(source => {
        const effect = structuredClone(source); effect.id = uuid(); effect.bakedRanges = [];
        if (!chain.includesAutomation) { effect.automation = []; effect.parameterAutomation = {}; effect.pitchEdits = []; effect.pitchEditsOnly = false; }
        else effect.pitchEdits = (effect.pitchEdits ?? []).map(r => ({ ...r, id: uuid(), points: (r.points ?? []).map(p => ({ ...p, id: uuid() })) }));
        return effect;
    });
}
interface LibraryDocument { version: number; presets: SavedEffectPreset[]; chains: SavedEffectChain[] }
export class EffectLibrary extends EventTarget {
    private document: LibraryDocument = { version: 1, presets: [], chains: [] };
    private history: string[] = [];
    error?: string;
    constructor(private storage: Pick<Storage, 'getItem' | 'setItem'>, private metadata: () => EffectMetadata[]) { super();
        try { const text = storage.getItem('soundroom.effectLibrary.v1'); if (text) { if (bytes(text) > MAX_LIBRARY) throw Error('Preset library exceeds 4 MiB.'); const d = record(parseJSON(text), ['version', 'presets', 'chains']); if (d.version !== 1 || !Array.isArray(d.presets) || !Array.isArray(d.chains) || d.presets.length > 128 || d.chains.length > 128) throw Error('Invalid preset library.'); const saved = d as unknown as LibraryDocument;
            for (const p of saved.presets) { const item=record(p,['id','name','kind','parameters','legacyAmount'],['id','name','kind','parameters']); id(item.id);presetName(item.name);validateModel(item.kind,'RealtimeEffectKind');validateModel(item.parameters,'[String: Float]');if(Object.keys(item.parameters as object).length>64||item.legacyAmount!=null&&(typeof item.legacyAmount!=='number'||!Number.isFinite(item.legacyAmount)||item.legacyAmount<0||item.legacyAmount>1))throw Error('Invalid stored effect preset.'); }
            unique(saved.presets,p=>`${p.kind}:${nameKey(presetName(p.name))}`);
            for (const c of saved.chains) { record(c,['id','name','effects','includesAutomation']);id(c.id);presetName(c.name);if(typeof c.includesAutomation!=='boolean'||!Array.isArray(c.effects)||c.effects.length>64)throw Error('Invalid stored chain.');for(const effect of c.effects)validateModel(effect,'RealtimeEffect'); }
            unique(saved.chains,c=>nameKey(presetName(c.name)));this.document=saved; } }
        catch (error) { this.error = String(error); }
    }
    presets(kind?: string) { return this.document.presets.filter(p => !kind || p.kind === kind).map(p => structuredClone(p)); }
    chains() { return this.document.chains.map(c => structuredClone(c)); }
    get canUndo() { return this.history.length > 0; }
    private change(next: LibraryDocument, remember = true) {
        if (this.error) throw Error('The existing preset library could not be loaded. Its storage is preserved.');
        if (next.presets.length > 128 || next.chains.length > 128) throw Error('The browser supports 128 presets and 128 chains.');
        if (next.presets.length) encodeEffectPresets(next.presets, this.metadata()); if (next.chains.length) encodeEffectChains(next.chains, this.metadata());
        const text = JSON.stringify(next); if (bytes(text) > MAX_LIBRARY) throw Error('The browser preset library is limited to 4 MiB.');
        this.storage.setItem('soundroom.effectLibrary.v1', text);
        if (remember) { this.history.push(JSON.stringify(this.document)); while (this.history.length > 20 || this.history.reduce((n, s) => n + bytes(s), 0) > 8_388_608) this.history.shift(); }
        this.document = structuredClone(next); this.dispatchEvent(new Event('change'));
    }
    savePreset(preset: SavedEffectPreset, replace?: string) { if(replace&&!this.document.presets.some(p=>p.id===replace))throw Error('That preset is no longer available.'); const p = checkedPreset(preset, this.metadata()); if (replace) p.id = replace; this.change({ ...this.document, presets: [...this.document.presets.filter(p => p.id !== replace), p] }); }
    saveChain(chain: SavedEffectChain, replace?: string) { if(replace&&!this.document.chains.some(c=>c.id===replace))throw Error('That chain is no longer available.'); const c = checkedChain(chain, this.metadata()); if (replace) c.id = replace; this.change({ ...this.document, chains: [...this.document.chains.filter(c => c.id !== replace), c] }); }
    rename(kind: 'preset' | 'chain', id: string, name: string) { name = presetName(name); this.change({ ...this.document, ...(kind === 'preset' ? { presets: this.document.presets.map(p => p.id === id ? { ...p, name } : p) } : { chains: this.document.chains.map(c => c.id === id ? { ...c, name } : c) }) }); }
    remove(kind: 'preset' | 'chain', id: string) { this.change({ ...this.document, ...(kind === 'preset' ? { presets: this.document.presets.filter(p => p.id !== id) } : { chains: this.document.chains.filter(c => c.id !== id) }) }); }
    undo() { const previous = this.history.at(-1); if (previous) { this.change(JSON.parse(previous) as LibraryDocument, false); this.history.pop(); } }
    import(text: string): { kind: 'preset' | 'chain'; count: number } {
        if (bytes(text) > MAX_CHAINS) throw Error('Preset files are limited to 4 MiB.');
        const probe = record(parseJSON(text), ['version', 'presets', 'chains'], ['version']);
        const kind = Object.hasOwn(probe, 'presets') ? 'preset' : 'chain', incoming = kind === 'preset' ? parseEffectPresets(text, this.metadata()) : parseEffectChains(text, this.metadata());
        const next = structuredClone(this.document);
        for (const p of incoming) { const list = kind === 'preset' ? next.presets : next.chains, base = p.name; let suffix = 2, name = base; while (list.some(item => nameKey(item.name) === nameKey(name) && (kind === 'chain' || (item as SavedEffectPreset).kind === (p as SavedEffectPreset).kind))) name = namePrefix(base, 68) + ` (${suffix++})`; const item = { ...p, id: uuid(), name }; if (kind === 'preset') next.presets.push(item as SavedEffectPreset); else next.chains.push(item as SavedEffectChain); }
        this.change(next); return { kind, count: incoming.length };
    }
}
