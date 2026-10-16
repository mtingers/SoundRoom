import type { SoundDesignPattern } from './generated/models.js';
/** Constant-tempo format-0 SMF. Instrument synthesis and articulations remain in Soundroom presets. */
export function exportMIDI(pattern: SoundDesignPattern): Uint8Array {
    const ppq = 960, bpm = pattern.bpm ?? 120, beats = pattern.beats ?? 4, notes = pattern.notes ?? [], controllers = pattern.controllers ?? [];
    if (!Number.isFinite(bpm) || bpm < 30 || bpm > 300 || !Number.isFinite(beats) || beats < 0.25 || beats > 64 || notes.length > 256 || controllers.length > 4096)
        throw Error('MIDI pattern is outside the supported bounds.');
    const events: {
        tick: number;
        order: number;
        data: number[];
    }[] = [];
    const tick = (beat: number) => { if (!Number.isFinite(beat) || beat < 0 || beat > beats)
        throw Error('MIDI event lies outside the pattern.'); return Math.round(beat * ppq); };
    const vlq = (n: number) => { if (!Number.isInteger(n) || n < 0 || n > 0x0fffffff)
        throw Error('MIDI delta is outside the supported range.'); const bytes = [n & 127]; while ((n >>>= 7) !== 0)
        bytes.unshift((n & 127) | 128); return bytes; };
    const tempo = Math.round(60000000 / bpm);
    events.push({ tick: 0, order: -10, data: [0xff, 0x51, 3, (tempo >>> 16) & 255, (tempo >>> 8) & 255, tempo & 255] });
    for (const n of notes) {
        if (!Number.isInteger(n.pitch) || n.pitch < 0 || n.pitch > 127 || !Number.isFinite(n.velocity) || n.velocity < 0 || n.velocity > 1 || !Number.isFinite(n.durationBeats) || n.durationBeats <= 0)
            throw Error('Invalid MIDI note.');
        if (n.velocity === 0)
            continue;
        const start = tick(n.startBeat), end = tick(n.startBeat + n.durationBeats);
        events.push({ tick: start, order: 2, data: [0x90, n.pitch, Math.max(1, Math.round(n.velocity * 127))] }, { tick: Math.max(start + 1, end), order: 0, data: [0x80, n.pitch, 0] });
    }
    for (const p of controllers) {
        if (!Number.isFinite(p.value) || p.value < 0 || p.value > 1)
            throw Error('Invalid MIDI controller.');
        let data: number[];
        if (p.kind === 'pitchBend') {
            const value = Math.round(p.value * 16383);
            data = [0xe0, value & 127, value >>> 7];
        }
        else {
            const cc = { sustain: 64, modulation: 1, expression: 11 }[p.kind];
            if (cc === undefined)
                throw Error('Unsupported MIDI controller.');
            data = [0xb0, cc, Math.round(p.value * 127)];
        }
        events.push({ tick: tick(p.beat), order: 1, data });
    }
    events.sort((a, b) => a.tick - b.tick || a.order - b.order);
    const body: number[] = [];
    let last = 0;
    for (const event of events) {
        body.push(...vlq(event.tick - last), ...event.data);
        last = event.tick;
    }
    body.push(...vlq(Math.max(0, tick(beats) - last)), 0xff, 0x2f, 0);
    const size = body.length;
    return Uint8Array.from([0x4d, 0x54, 0x68, 0x64, 0, 0, 0, 6, 0, 0, 0, 1, ppq >>> 8, ppq & 255, 0x4d, 0x54, 0x72, 0x6b, (size >>> 24) & 255, (size >>> 16) & 255, (size >>> 8) & 255, size & 255, ...body]);
}
