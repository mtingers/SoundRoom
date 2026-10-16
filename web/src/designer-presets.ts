import {randomUUID} from './crypto.js';
import type { DesignerSoundPreset, SoundDesignPattern } from './generated/models.js';
import { validateModel } from './validate.js';
const max64 = 18446744073709551615n;
/** Convert only exact native UInt64 fields, with a JSON lexer so text containing field names is untouched. */
function transformSeedFields(text: string, encode: boolean): string {
    let output = '', position = 0;
    while (position < text.length) {
        if (text[position] !== '"') {
            output += text[position++];
            continue;
        }
        let end = position + 1, escaped = false;
        while (end < text.length) {
            const c = text[end++]!;
            if (escaped)
                escaped = false;
            else if (c === '\\')
                escaped = true;
            else if (c === '"')
                break;
        }
        const token = text.slice(position, end);
        output += token;
        position = end;
        let next = position;
        while (/\s/.test(text[next] ?? 'x'))
            next++;
        if (text[next] !== ':' || !['seed', 'organicSeed'].includes(JSON.parse(token)))
            continue;
        output += text.slice(position, next + 1);
        position = next + 1;
        while (/\s/.test(text[position] ?? 'x'))
            output += text[position++];
        let value: string | undefined;
        if (encode && text[position] === '"') {
            const close = text.indexOf('"', position + 1);
            if (close < 0)
                throw Error('Invalid seed.');
            value = text.slice(position + 1, close);
            position = close + 1;
        }
        else if (!encode && /[0-9-]/.test(text[position] ?? 'x')) {
            const begin = position;
            while (/[0-9.eE+-]/.test(text[position] ?? 'x'))
                position++;
            value = text.slice(begin, position);
        }
        if (value !== undefined) {
            if (!/^\d{1,20}$/.test(value) || BigInt(value) > max64)
                throw Error('Seed must be an exact unsigned 64-bit integer.');
            output += encode ? value : JSON.stringify(value);
        }
    }
    return output;
}
const defaults: SoundDesignPattern = { instrument: 'synthesizer', character: 0.5, resonance: 0.5, bpm: 120, beats: 4, waveform: 'triangle', secondWaveform: 'saw', tone: 0.65, attack: 0.01, release: 0.15, decay: 0.1, sustain: 1, oscillatorMix: 0, detune: 0, pulseWidth: 0.5, subLevel: 0, noiseLevel: 0, drive: 1, baseGain: 1, basePan: 0, notes: [], controllers: [], layers: [], organic: { drift: 0, timbre: 0, breath: 0, seed: '17' } };
function normalizedSound(sound: SoundDesignPattern): SoundDesignPattern { if ((sound.layers?.length ?? 0) > 6)
    throw Error('Sounds support up to six layers.'); const value = { ...structuredClone(defaults), ...structuredClone(sound), organic: { ...defaults.organic, ...sound.organic }, layers: (sound.layers ?? []).map(layer => ({ id: randomUUID().toUpperCase(), preset: 'Glass pluck' as const, gain: 0.5, pan: 0, transpose: 0, minimumVelocity: 0, maximumVelocity: 1, enabled: true, ...layer })) }; return value; }
export function soundPreset(pattern: SoundDesignPattern): DesignerSoundPreset { const value = { version: 1, sound: { ...structuredClone(pattern), notes: [], controllers: [], bpm: 120, beats: 4 } }; validateModel(value, 'DesignerSoundPreset'); return value; }
export function applySoundPreset(preset: DesignerSoundPreset, sequence: SoundDesignPattern): SoundDesignPattern { validateModel(preset, 'DesignerSoundPreset'); if (preset.version !== 1)
    throw Error('Unsupported sound preset version.'); return { ...normalizedSound(preset.sound), notes: structuredClone(sequence.notes ?? []), controllers: structuredClone(sequence.controllers ?? []), bpm: sequence.bpm, beats: sequence.beats }; }
export function parseSoundPreset(text: string): DesignerSoundPreset { if (new TextEncoder().encode(text).length > 524288)
    throw Error('Sound presets are limited to 512 KiB.'); const value: unknown = JSON.parse(transformSeedFields(text, false)); validateModel(value, 'DesignerSoundPreset'); if ((value as DesignerSoundPreset).version !== 1)
    throw Error('Unsupported sound preset version.'); return value as DesignerSoundPreset; }
export function encodeSoundPreset(pattern: SoundDesignPattern): string { return transformSeedFields(JSON.stringify(soundPreset(pattern), null, 2), true); }
