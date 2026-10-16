import {randomUUID} from './crypto.js';
import { WorkspaceSource, type WorkspaceSourceAccess } from './workspace-source.js';
import { acceptWorkspaceShortcut, bindInstrumentKey, isItemNavigation, navigateTimelineItem, openWorkspaceDialog } from './input-focus.js';
import { remapDrumGrid, resizeDrumBars, parseDrumDocument, duplicateDrumPattern, validateDrumPattern, type DrumDocument } from './beat-editing.js';
import type { DrumPattern as WireDrumPattern } from './generated/models.js';
import { ControllerEditor } from './controller-editor.js';
import { soundPreset, applySoundPreset, parseSoundPreset, encodeSoundPreset } from './designer-presets.js';
import type { DesignerSoundPreset } from './generated/models.js';
import { exportMIDI } from './midi-export.js';
import type { SoundDesignPattern } from './generated/models.js';
import { noteName } from './music.js';
export { noteName } from './music.js';
import { type Command, type Track, type Timing } from './model.js';
import { encodeBase64 } from './sdk.js';
import { GestureWriter } from './automation.js';
import { MIDIExcerpt, type MIDIContent } from './midi.js';
export interface Note {
    id: string;
    pitch: number;
    velocity: number;
    startBeat: number;
    durationBeats: number;
    articulation?: string;
}
export interface Controller {
    kind: 'sustain' | 'modulation' | 'expression' | 'pitchBend';
    beat: number;
    value: number;
}
export interface Pattern {
    notes: Note[];
    controllers: Controller[];
    layers: Layer[];
    organic: {
        drift: number;
        timbre: number;
        breath: number;
        seed: string;
    };
    bpm: number;
    beats: number;
    instrument: string;
    [key: string]: unknown;
}
interface Layer {
    id: string;
    preset: string;
    gain: number;
    pan: number;
    transpose: number;
    minimumVelocity: number;
    maximumVelocity: number;
    enabled: boolean;
    [key: string]: unknown;
}
export interface DesignerPreset {
    id: string;
    name: string;
    category: string;
    sound: Pattern;
}
export interface Instrument {
    id: string;
    name: string;
    characterLabel: string;
}
interface DrumStep {
    voice: string;
    index: number;
    velocity: number;
    pan: number;
    probability: number;
    articulation?: string;
}
interface DrumPattern {
    name: string;
    kit: string;
    bpm: number;
    bars: number;
    stepsPerBeat: number;
    swing: number;
    humanize: number;
    masterGain: number;
    velocityHumanize?: number;
    variationCycles?: number;
    steps: DrumStep[];
    lanes: Record<string, {
        kit: string;
        gain: number;
        pan: number;
        tuning: number;
        muted: boolean;
        damping?: number;
    }>;
}
export interface DrumKit {
    id: string;
    name: string;
    voices: string[];
    articulations: Record<string, string[]>;
}
export interface DrumPreset {
    id: string;
    name: string;
    pattern: DrumPattern;
}
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, attributes: Record<string, string> = {}, text?: string) => {
    const e = document.createElement(tag);
    for (const [key, value] of Object.entries(attributes))
        e.setAttribute(key, value);
    if (text)
        e.textContent = text;
    return e;
};
const clamp = (v: number, a: number, b: number) => Math.min(b, Math.max(a, v));
export class CreativeWorkspaces {
    private dialog: HTMLDialogElement;
    private toolbar: HTMLElement;
    private controls: HTMLElement;
    private content: HTMLElement;
    private status: HTMLElement;
    private current: 'designer' | 'beats' = 'designer';
    private pattern?: Pattern;
    private drums?: DrumPattern;
    private presets: DesignerPreset[] = [];
    private instruments: Instrument[] = [];
    private kits: DrumKit[] = [];
    private drumPresets: DrumPreset[] = [];
    private previewing = false;
    private auditionOwner?: string;
    private previewWriter?: GestureWriter<Pattern | DrumPattern>;
    private undo: (Pattern | DrumPattern | DrumDocument)[] = [];
    private redo: (Pattern | DrumPattern | DrumDocument)[] = [];
    private selected = new Set<string>();
    private canvas?: HTMLCanvasElement;
    private offset = 0;
    private beatPixels = 90;
    private topPitch = 84;
    private playingPosition = 0;
    private noteDrag?: {
        id: string;
        original: Note;
        x: number;
        y: number;
        resize: boolean;
    };
    private noteGestureOriginal?: Pattern;
    private resize?: ResizeObserver;
    private raf = 0;
    private dirty = true;
    private midi?: MIDIContent;
    private midiWorker?: Worker;
    private keys = new Map<string, number>();
    private pointerNotes = new Set<number>();
    private focusedKeys = new Set<number>();
    private recordingNotes = false;
    private activeRecorded = new Map<number, {
        beat: number;
        time: number;
        velocity: number;
    }>();
    private selectedStep?: DrumStep;
    private enabled = false;
    private canOperate = false;
    private editingClipID?: string;
    private source?: WorkspaceSource;
    private generation=0;
    private busy=false;
    private draftVersion=0;
    private serverMIDISources: {
        id: number;
        name: string;
    }[] = [];
    private midiSourceID = 0;
    private midiWriter?: GestureWriter<Pattern>;
    private recordStarted = 0;
    private keyboardBase = 60;
    private midiConnectGeneration = 0;
    private pendingMIDI?: Promise<unknown>;
    private controllerEditor: ControllerEditor;
    private beatBank?: DrumDocument;
    private beatClipboard?: DrumPattern;
    constructor(private send: (command: Command, trackID?: string, expectedTrackRevision?:number, expectedSessionRevision?:number) => Promise<unknown>, private track: () => Track | undefined, private cursor: () => number, private timing: () => Timing, private clientID: () => string, private clip: () => {
        id: string;
        start: number;
        soundDesign?: unknown;
        drumPattern?: unknown;
    } | undefined, private sourceAccess?: WorkspaceSourceAccess) {
        this.dialog = el('dialog', { class: 'creative-dialog' });
        this.toolbar = el('div', { class: 'creative-toolbar' });
        this.controls = el('div', { class: 'creative-controls' });
        this.content = el('div', { class: 'creative-content' });
        this.status = el('div', { class: 'creative-status', role: 'status' });
        this.dialog.append(this.toolbar, this.controls, this.content, this.status);
        document.body.append(this.dialog);
        this.dialog.setAttribute('aria-label','Sound Designer and Beats');
        this.dialog.oncancel = () => void this.close();
        this.dialog.addEventListener('keydown', e => this.keyDown(e));
        this.dialog.addEventListener('keyup', e => this.keyUp(e));
        window.addEventListener('blur', () => this.releaseNotes());
        document.addEventListener('visibilitychange', () => {
            if (document.hidden)
                this.releaseNotes();
        });
        this.controllerEditor = new ControllerEditor(() => this.pattern as unknown as SoundDesignPattern, () => this.snapshot(), () => this.changed(), () => this.enabled, () => void this.togglePreview());
        this.raf = requestAnimationFrame(() => this.tick());
    }
    configure(capabilities: Record<string, unknown>, enabled: boolean, canOperate: boolean) {
        this.canOperate = canOperate;
        this.presets = (capabilities.designerPresets as DesignerPreset[]) ?? [];
        this.instruments = (capabilities.instruments as Instrument[]) ?? [];
        this.kits = (capabilities.drumKits as DrumKit[]) ?? [];
        this.drumPresets = (capabilities.drumPresets as DrumPreset[]) ?? [];
        this.enabled = enabled;
        if (this.dialog.open) {
            for (const control of this.dialog.querySelectorAll<HTMLInputElement | HTMLButtonElement | HTMLSelectElement>('input,select,button'))
                control.disabled = !enabled;
            const play = this.toolbar.querySelector<HTMLButtonElement>('[data-play]');
            if (play)
                play.disabled = !canOperate;
            for (const key of this.dialog.querySelectorAll<HTMLButtonElement>('.virtual-keyboard button'))
                key.disabled = !canOperate;
            for (const control of this.dialog.querySelectorAll<HTMLInputElement | HTMLButtonElement | HTMLSelectElement>('[data-operator]'))
                control.disabled = !canOperate;
            const insert=this.toolbar.querySelector<HTMLButtonElement>('[data-commit]');if(insert)insert.disabled=!enabled||this.busy||!!this.source&&!this.source.ready();
            for(const button of this.toolbar.querySelectorAll<HTMLButtonElement>('[data-source]'))button.disabled=!enabled||this.busy;
            if(!this.busy&&this.source&&!this.source.ready())this.status.textContent='The target changed. Your private draft is retained; reload the source or insert it on a new track.';
            const close = this.toolbar.lastElementChild as HTMLButtonElement;
            if (close)
                close.disabled = false;
        }
    }
    open(kind: 'designer' | 'beats', pinned?:WorkspaceSource) {
        if (!this.enabled) {
            this.status.textContent = 'Connect to an editor-enabled server.';
            return;
        }
        if (kind === 'designer' && !this.presets.length || kind === 'beats' && !this.kits.length) {
            alert('This server does not advertise this workspace.');
            return;
        }
        this.generation++;this.busy=false;this.draftVersion=0;
        this.current = kind;
        this.previewing = false;
        this.recordingNotes = false;
        this.activeRecorded.clear();
        this.midiSourceID = 0;
        this.midiConnectGeneration++;
        this.undo = [];
        this.redo = [];
        this.selected.clear();
        this.offset = 0;
        const selected = pinned ? (pinned.trackID?this.sourceAccess?.track(pinned.trackID)?.clips.find(c=>c.id===pinned.clipID):undefined) : this.clip(), clip = selected && (selected as {
            soundDesign?: Pattern;
            drumPattern?: DrumPattern;
        })[kind === 'designer' ? 'soundDesign' : 'drumPattern'] ? selected : undefined;
        this.editingClipID = clip?.id;
        this.source=new WorkspaceSource(pinned?pinned.trackID:this.track()?.id,clip?.id,clip?.start??pinned?.frame??this.cursor(),this.sourceAccess);
        if (kind === 'designer') {
            this.pattern = structuredClone((clip as unknown as {
                soundDesign?: Pattern;
            } | undefined)?.soundDesign ?? this.presets[0]!.sound);
            if(!clip)this.pattern.bpm = this.timing().tempo;
            this.pattern.notes ??= [];
            this.pattern.controllers ??= [];
            this.pattern.layers ??= [];
        }
        else
            this.drums = structuredClone((clip as unknown as {
                drumPattern?: DrumPattern;
            } | undefined)?.drumPattern ?? this.drumPresets[0]?.pattern ?? { name: 'Drum loop', kit: this.kits[0]!.id, bpm: this.timing().tempo, bars: 1, stepsPerBeat: 4, swing: 0, humanize: 0, masterGain: 0.8, steps: [], lanes: {} });
        if (kind === 'beats') {
            if(!clip)this.drums!.bpm=this.timing().tempo;
            this.beatBank = { version: 2, patterns: [this.drums as unknown as WireDrumPattern], selected: 0 };
        }
        this.previewWriter = new GestureWriter(async (pattern) => !!await this.intent(kind === 'designer' ? 'designer.preview' : 'drums.preview', pattern));
        this.render();
        openWorkspaceDialog(this.dialog,this.canvas);
        this.dirty = true;
    }
    async close() {
        const generation=++this.generation;
        this.releaseNotes();
        this.previewWriter?.cancel();
        this.midiWriter?.cancel();
        const pending = this.pendingMIDI;
        this.midiConnectGeneration++;
        if (pending)
            await pending;
        if(generation!==this.generation)return;
        if (this.previewing || this.midiSourceID || pending)
            await this.intent('audition.stop', undefined, {}, {}, { kind: this.current === 'designer' ? 'designer' : 'drums' });
        if(generation!==this.generation)return;
        this.previewing = false;
        this.midiSourceID = 0;
        this.midiConnectGeneration++;
        this.resize?.disconnect();
        this.dialog.close();
    }
    audition(event: {
        kind: string;
        ownerID?: string;
        playing: boolean;
        preparing: boolean;
        position: number;
    }) {
        if (event.kind === (this.current === 'designer' ? 'designer' : 'drums')) {
            this.auditionOwner = event.ownerID;
            this.previewing = event.playing;
            this.playingPosition = event.position;
            this.dirty = true;
            const button = this.toolbar.querySelector<HTMLButtonElement>('[data-play]');
            if (button)
                button.textContent = event.preparing ? 'Preparing…' : event.playing ? 'Ⅱ Pause' : '▶ Audition';
        }
    }
    private intent(operation: string, pattern?: Pattern | DrumPattern, integers: Record<string, number> = {}, numbers: Record<string, number> = {}, strings: Record<string, string> = {}) {
        const target=this.source?.trackID,frame=this.source?.frame??this.cursor();
        return this.send({type:'editor',intent:{operation,context:{trackID:target,cursorFrame:frame,clipIDs:[]},integers,numbers,strings,flags:{},payload:pattern?encodeBase64(new TextEncoder().encode(JSON.stringify(pattern))):undefined}},target);
    }
    private snapshot() {
        const value = this.current === 'designer' ? this.pattern : this.beatDocument();
        if (value) {
            this.undo.push(structuredClone(value));
            if (this.undo.length > 100)
                this.undo.shift();
            this.redo = [];
        }
    }
    private changed() { this.draftVersion++;
        this.dirty = true;
        this.controllerEditor?.invalidate();
        if (this.current === 'designer' && this.midiSourceID)
            this.midiWriter?.offer(structuredClone(this.pattern!));
        if (this.previewing && this.auditionOwner?.toUpperCase() === this.clientID().toUpperCase())
            this.previewWriter?.offer(structuredClone(this.current === 'designer' ? this.pattern! : this.drums!));
    }
    private button(text: string, action: () => void, attributes: Record<string, string> = {}) { const b = el('button', attributes, text); b.onclick = action; return b; }
    private numeric(parent: HTMLElement, name: string, key: string, min: number, max: number, step: number, object: Record<string, unknown>, change = () => this.changed()) {
        const label = el('label', {}, name), input = el('input', { type: 'number', min: String(min), max: String(max), step: String(step), 'aria-label': name });
        input.value = String(object[key] ?? min);
        input.onchange = () => {
            const value = Number(input.value);
            if (!Number.isFinite(value) || value < min || value > max || step === 1 && !Number.isInteger(value)) {
                this.status.textContent = `${name} must be ${min}–${max}.`;
                return;
            }
            this.snapshot();
            object[key] = value;
            if (key === 'minimumVelocity')
                object.maximumVelocity = Math.max(value, Number(object.maximumVelocity ?? 1));
            if (key === 'maximumVelocity')
                object.minimumVelocity = Math.min(value, Number(object.minimumVelocity ?? 0));
            change();
        };
        label.append(input);
        parent.append(label);
        return input;
    }
    private render() {
        this.resize?.disconnect();
        this.toolbar.replaceChildren();
        this.controls.replaceChildren();
        this.content.replaceChildren();
        const heading = el('strong', {}, this.current === 'designer' ? 'Sound designer' : 'Beats');
        const play = this.button('▶ Audition', () => void this.togglePreview(), { 'data-play': '', 'title': 'Space plays/pauses this workspace' });
        const insert = this.button(this.editingClipID?'Replace clip':'Insert at position', () => void this.commit(), { class: 'primary', 'data-commit':'' });insert.disabled=!this.enabled||this.busy||!!this.source&&!this.source.ready();
        const close = this.button('×', () => void this.close(), { 'aria-label': 'Close creative workspace' });
        play.disabled = !this.canOperate;
        this.toolbar.append(heading, play, this.button('Undo', () => this.localUndo(false)), this.button('Redo', () => this.localUndo(true)), insert,this.button('Reload source',()=>void this.reloadSource(),{'data-source':''}),this.button('Insert as new track',()=>void this.insertNewTrack(),{'data-source':''}),close);
        for(const b of this.toolbar.querySelectorAll<HTMLButtonElement>('[data-source]'))b.disabled=!this.enabled||this.busy;
        this.status.textContent = 'Audio plays on the server. Your edits stay private until insertion.';
        if (this.current === 'designer')
            this.renderDesigner();
        else
            this.renderBeats();
    }
    private async togglePreview() {
        if (!this.canOperate) {
            this.status.textContent = 'Audition requires an operator role; editors can edit and insert patterns.';
            return;
        }
        if (this.previewing) {
            await this.intent('audition.stop', undefined, {}, {}, { kind: this.current === 'designer' ? 'designer' : 'drums' });
            this.previewing = false;
            return;
        }
        if (await this.intent(this.current === 'designer' ? 'designer.preview' : 'drums.preview', this.current === 'designer' ? this.pattern : this.drums)) {
            this.previewing = true;
            this.auditionOwner = this.clientID();
        }
    }
    private async reloadSource() {
        if(this.busy||!confirm('Discard this private draft and reload its source from the server?'))return;
        const source=this.source,kind=this.current,generation=this.generation,track=source?.trackID?this.sourceAccess?.track(source.trackID):undefined,clip=track?.clips.find(c=>c.id===source?.clipID);
        if(source?.clipID&&(!clip||!(clip as unknown as Record<string,unknown>)[kind==='designer'?'soundDesign':'drumPattern'])){this.status.textContent='The editable source clip is no longer available. Keep this draft or insert it on a new track.';return;}
        await this.close();if(this.generation!==generation+1)return;this.open(kind,source);
    }
    private async insertNewTrack() { if(this.busy)return;this.source=new WorkspaceSource(undefined,undefined,this.source?.frame??this.cursor(),this.sourceAccess);this.editingClipID=undefined;await this.commit(); }
    private async commit() {
        if(this.busy||!this.enabled)return;
        if(this.source&&!this.source.ready()){this.status.textContent='The target changed. Your draft is retained; reload the source or insert it on a new track.';return;}
        if(this.current==='designer'&&!this.pattern?.notes.length||this.current==='beats'&&!this.drums?.steps.length){this.status.textContent=this.current==='designer'?'Add notes before inserting.':'Add drum hits before inserting.';return;}
        const generation=this.generation,version=this.draftVersion,source=this.source,pattern=structuredClone(this.current==='designer'?this.pattern:this.drums),operation=this.current==='designer'?'designer.commit':'drums.commit';
        this.busy=true;this.render();
        try {
            if(this.previewWriter&&!await this.previewWriter.flush()||this.midiWriter&&!await this.midiWriter.flush())return;
            if(generation!==this.generation)return;
            if(source&&!source.ready()){this.status.textContent='The target changed while preparing. Your draft is retained; reload the source or insert it on a new track.';return;}
            const reply=await this.send({type:'editor',intent:{operation,context:{trackID:source?.trackID,cursorFrame:source?.frame??this.cursor(),clipIDs:[]},integers:{},numbers:{},strings:{trackID:source?.trackID??'',...(source?.clipID?{clipID:source.clipID}:{})},flags:{},payload:encodeBase64(new TextEncoder().encode(JSON.stringify(pattern)))}},source?.trackID,source?.trackRevision,source?.trackID?undefined:source?.sessionRevision);
            if(generation!==this.generation)return;
            if(!reply){this.status.textContent='Insertion was not confirmed. Your draft is retained; review the server status before retrying.';return;}
            if(source&&!source.accepted(reply)){this.status.textContent='The target changed after insertion. Your draft is retained for review.';return;}
            this.previewing=false;
            if(version!==this.draftVersion){this.status.textContent='The earlier pattern was inserted. Your newer private draft remains here.';return;}
            this.dialog.close();
        }finally{if(generation===this.generation){this.busy=false;const insert=this.toolbar.querySelector<HTMLButtonElement>('[data-commit]');if(insert)insert.disabled=!this.enabled||!!this.source&&!this.source.ready();for(const b of this.toolbar.querySelectorAll<HTMLButtonElement>('[data-source]'))b.disabled=!this.enabled;}}
    }
    private exportMIDI() {
        try {
            const bytes = exportMIDI(this.pattern as unknown as SoundDesignPattern), url = URL.createObjectURL(new Blob([bytes.buffer as ArrayBuffer], { type: 'audio/midi' })), link = el('a');
            link.href = url;
            link.download = String(this.pattern?.instrument ?? 'Soundroom notes').replace(/[\/:*?"<>|]/g, '-') + '.mid';
            link.click();
            setTimeout(() => URL.revokeObjectURL(url), 10000);
            this.status.textContent = 'MIDI notes, velocity, tempo and controllers exported. Soundroom synthesis stays in the sound preset.';
        }
        catch (error) {
            this.status.textContent = error instanceof Error ? error.message : 'MIDI export failed.';
        }
    }
    private renderDesigner() {
        const pattern = this.pattern!, preset = el('select', { 'aria-label': 'Designer preset' });
        preset.add(new Option('Choose sound preset…', ''));
        for (const p of this.presets)
            preset.add(new Option(`${p.category === 'organic' ? 'Organic · ' : ''}${p.name}`, p.id));
        preset.onchange = () => {
            const p = this.presets.find(p => p.id === preset.value);
            if (p) {
                this.snapshot();
                this.pattern = { ...structuredClone(p.sound), notes: pattern.notes, controllers: pattern.controllers, bpm: pattern.bpm, beats: pattern.beats };
                this.render();
                this.changed();
            }
        };
        this.toolbar.insertBefore(preset, this.toolbar.lastElementChild);
        this.toolbar.insertBefore(this.button('Export MIDI', () => this.exportMIDI()), this.toolbar.lastElementChild);
        this.toolbar.insertBefore(this.button('MIDI import', () => this.importMIDI()), this.toolbar.lastElementChild);
        this.toolbar.insertBefore(this.button('−', () => { this.beatPixels = Math.max(20, this.beatPixels / 1.5); this.dirty = true; }, { 'aria-label': 'Zoom out piano roll' }), this.toolbar.lastElementChild);
        this.toolbar.insertBefore(this.button('+', () => { this.beatPixels = Math.min(480, this.beatPixels * 1.5); this.dirty = true; }, { 'aria-label': 'Zoom in piano roll' }), this.toolbar.lastElementChild);
        const sound = el('details', { open: '' }), summary = el('summary', {}, 'Instrument & envelope');
        sound.append(summary);
        const instrument = el('select', { 'aria-label': 'Native instrument' });
        for (const i of this.instruments)
            instrument.add(new Option(i.name, i.id));
        instrument.value = pattern.instrument;
        instrument.onchange = () => { this.snapshot(); pattern.instrument = instrument.value; this.changed(); };
        sound.append(instrument);
        for (const [name, key, min, max, step] of [['Character', 'character', 0, 1, 0.01], ['Resonance', 'resonance', 0, 1, 0.01], ['Tone', 'tone', 0, 1, 0.01], ['Attack (s)', 'attack', 0, 2, 0.001], ['Decay (s)', 'decay', 0, 2, 0.001], ['Sustain', 'sustain', 0, 1, 0.01], ['Release (s)', 'release', 0, 2, 0.001], ['Drive', 'drive', 1, 8, 0.01], ['Base gain', 'baseGain', 0, 1, 0.01], ['Base pan', 'basePan', -1, 1, 0.01], ['Beats', 'beats', 0.25, 64, 0.25], ['BPM', 'bpm', 30, 300, 0.1]] as const)
            this.numeric(sound, name, key, min, max, step, pattern);
        this.controls.append(sound);
        const oscillators = el('details'), os = el('summary', {}, 'Oscillators');
        oscillators.append(os);
        for (const key of ['waveform', 'secondWaveform']) {
            const label = el('label', {}, key === 'waveform' ? 'Oscillator 1' : 'Oscillator 2'), select = el('select', { 'aria-label': label.textContent! });
            for (const waveform of ['sine', 'triangle', 'saw', 'pulse'])
                select.add(new Option(waveform, waveform));
            select.value = String(pattern[key]);
            select.onchange = () => { this.snapshot(); pattern[key] = select.value; this.changed(); };
            label.append(select);
            oscillators.append(label);
        }
        for (const [name, key, min, max, step] of [['Oscillator mix', 'oscillatorMix', 0, 1, 0.01], ['Detune (cents)', 'detune', -50, 50, 0.1], ['Pulse width', 'pulseWidth', 0.05, 0.95, 0.01], ['Sub level', 'subLevel', 0, 1, 0.01], ['Noise level', 'noiseLevel', 0, 1, 0.01]] as const)
            this.numeric(oscillators, name, key, min, max, step, pattern);
        this.controls.append(oscillators);
        const organic = el('details'), og = el('summary', {}, 'Organic motion');
        organic.append(og);
        for (const [name, key] of [['Pitch drift', 'drift'], ['Timbre variation', 'timbre'], ['Breath', 'breath']] as const)
            this.numeric(organic, name, key, 0, 1, 0.01, pattern.organic);
        const seed = el('label', {}, 'Seed'), input = el('input', { 'aria-label': 'Organic seed', inputmode: 'numeric' });
        input.value = String(pattern.organic.seed);
        input.onchange = () => {
            if (!/^\d{1,20}$/.test(input.value) || BigInt(input.value) > 18446744073709551615n) {
                this.status.textContent = 'Seed must be an unsigned 64-bit integer.';
                return;
            }
            this.snapshot();
            pattern.organic.seed = input.value;
            this.changed();
        };
        seed.append(input);
        organic.append(seed);
        this.controls.append(organic);
        const layers = el('details'), ls = el('summary', {}, 'Layers');
        layers.append(ls);
        for (const layer of pattern.layers) {
            const card = el('div', { class: 'layer-card' }), choose = el('select', { 'aria-label': 'Layer sound' });
            for (const p of this.presets.filter(p => p.category === 'instrument'))
                choose.add(new Option(p.name, p.id.replace('factory.', '')));
            choose.value = layer.preset;
            choose.onchange = () => { this.snapshot(); layer.preset = choose.value; this.changed(); };
            const enabled = el('label', {}, 'Enabled'), toggle = el('input', { type: 'checkbox' });
            toggle.checked = layer.enabled !== false;
            toggle.onchange = () => { this.snapshot(); layer.enabled = toggle.checked; this.changed(); };
            enabled.append(toggle);
            card.append(choose, enabled);
            for (const [name, key, min, max, step] of [['Gain', 'gain', 0, 1, 0.01], ['Pan', 'pan', -1, 1, 0.01], ['Transpose', 'transpose', -24, 24, 1], ['Low velocity', 'minimumVelocity', 0, 1, 0.01], ['High velocity', 'maximumVelocity', 0, 1, 0.01], ['Velocity fade', 'velocityFade', 0, 0.5, 0.01]] as const)
                this.numeric(card, name, key, min, max, step, layer);
            this.renderLayerOverrides(card, layer);
            card.append(this.button('Remove layer', () => { this.snapshot(); pattern.layers = pattern.layers.filter(l => l.id !== layer.id); this.render(); this.changed(); }));
            layers.append(card);
        }
        if (pattern.layers.length < 6)
            layers.append(this.button('Add layer', () => { this.snapshot(); pattern.layers.push({ id: randomUUID().toUpperCase(), preset: this.presets.find(p => p.category === 'instrument')!.id.replace('factory.', ''), gain: 0.5, pan: 0, transpose: 0, minimumVelocity: 0, maximumVelocity: 1, enabled: true }); this.render(); this.changed(); }));
        this.controls.append(layers);
        this.canvas = el('canvas', { tabindex: '0', role: 'application', 'aria-label': 'Piano roll. Home or End selects the first or last note; brackets select previous or next. Control or Command A selects all notes. Click to add notes, drag to move or resize at the right edge. Arrow keys move selected notes; Shift arrows resize. Delete removes notes.' });
        this.content.append(this.canvas);
        this.canvas.onpointerdown = e => this.noteDown(e);
        this.canvas.onpointermove = e => this.noteMove(e);
        this.canvas.onpointerup = e => {
            this.noteDrag = undefined;
            this.noteGestureOriginal = undefined;
            if (this.canvas?.hasPointerCapture(e.pointerId))
                this.canvas.releasePointerCapture(e.pointerId);
        };
        this.canvas.onwheel = e => {
            e.preventDefault();
            if (e.ctrlKey || e.metaKey)
                this.beatPixels = clamp(this.beatPixels * Math.exp(-e.deltaY * 0.004), 20, 480);
            else if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY))
                this.offset = Math.max(0, this.offset + (e.shiftKey ? e.deltaY : e.deltaX));
            else
                this.topPitch = clamp(this.topPitch + Math.sign(e.deltaY) * 2, 24, 127);
            this.dirty = true;
        };
        this.resize = new ResizeObserver(() => { const canvas = this.canvas!, dpr = Math.min(devicePixelRatio, 2); canvas.width = this.content.clientWidth * dpr; canvas.height = Math.max(200, this.content.clientHeight - 95) * dpr; canvas.style.width = `${this.content.clientWidth}px`; canvas.style.height = `${Math.max(200, this.content.clientHeight - 95)}px`; canvas.getContext('2d')!.setTransform(dpr, 0, 0, dpr, 0, 0); this.dirty = true; });
        this.resize.observe(this.content);
        this.renderKeyboard();
        this.renderControllers();
        this.renderMIDISources();
        this.renderPresetManager();
    }
    private customSounds(): {
        name: string;
        preset: DesignerSoundPreset;
    }[] {
        try {
            const raw = localStorage.getItem('soundroom.designer.presets') ?? '[]';
            if (raw.length > 2097152)
                return [];
            const values = JSON.parse(raw);
            return Array.isArray(values) ? values.filter(p => {
                try {
                    if (typeof p.name !== 'string' || !p.name.trim() || new TextEncoder().encode(p.name).length > 128)
                        return false;
                    applySoundPreset(p.preset, {});
                    return true;
                }
                catch {
                    return false;
                }
            }).slice(-64) : [];
        }
        catch {
            return [];
        }
    }
    private storeSound(name: string, preset: DesignerSoundPreset) {
        name = name.trim();
        if (!name || new TextEncoder().encode(name).length > 128)
            throw Error('Preset names must be 1–128 UTF-8 bytes.');
        const saved = [...this.customSounds().filter(p => p.name !== name), { name, preset }].slice(-64), raw = JSON.stringify(saved);
        if (raw.length > 2097152)
            throw Error('Custom sounds exceed the browser storage limit.');
        localStorage.setItem('soundroom.designer.presets', raw);
        this.renderPresetManager();
        this.status.textContent = 'Sound preset saved in this browser.';
    }
    private renderPresetManager() {
        this.controls.querySelector('[data-sound-presets]')?.remove();
        const panel = el('details', { 'data-sound-presets': '' });
        panel.append(el('summary', {}, 'Custom sound presets'));
        const choose = el('select', { 'aria-label': 'Custom sound preset' });
        choose.add(new Option('Choose custom sound…', ''));
        for (const p of this.customSounds())
            choose.add(new Option(p.name, p.name));
        choose.onchange = () => {
            const preset = this.customSounds().find(p => p.name === choose.value);
            if (preset) {
                this.snapshot();
                this.pattern = applySoundPreset(preset.preset, this.pattern as unknown as SoundDesignPattern) as unknown as Pattern;
                this.render();
                this.changed();
            }
        };
        const save = this.button('Save current sound', () => {
            const name = prompt('Sound preset name');
            if (name !== null)
                try {
                    this.storeSound(name, soundPreset(this.pattern as unknown as SoundDesignPattern));
                }
                catch (error) {
                    this.status.textContent = String(error);
                }
        }), remove = this.button('Delete selected', () => {
            if (!choose.value)
                return;
            try {
                localStorage.setItem('soundroom.designer.presets', JSON.stringify(this.customSounds().filter(p => p.name !== choose.value)));
                this.renderPresetManager();
            }
            catch {
                this.status.textContent = 'Browser storage is unavailable.';
            }
        }), load = this.button('Import sound file', () => {
            const file = el('input', { type: 'file', accept: '.soundroom-preset.json,.soundroom-sound,.json' });
            file.onchange = () => {
                const selected = file.files?.[0];
                if (!selected)
                    return;
                if (selected.size > 524288) {
                    this.status.textContent = 'Sound files are limited to 512 KiB.';
                    return;
                }
                void selected.text().then(text => { const preset = parseSoundPreset(text); this.snapshot(); this.pattern = applySoundPreset(preset, this.pattern as unknown as SoundDesignPattern) as unknown as Pattern; this.render(); this.changed(); this.status.textContent = 'Sound loaded; notes, controllers and tempo retained.'; }).catch(error => this.status.textContent = String(error));
            };
            file.click();
        }), exportSound = this.button('Export sound file', () => {
            try {
                const text = encodeSoundPreset(this.pattern as unknown as SoundDesignPattern), url = URL.createObjectURL(new Blob([text], { type: 'application/json' })), link = el('a');
                link.href = url;
                link.download = String(this.pattern?.instrument ?? 'Soundroom') + '.soundroom-preset.json';
                link.click();
                setTimeout(() => URL.revokeObjectURL(url), 10000);
                this.status.textContent = 'Native sound preset exported with exact organic seeds.';
            }
            catch (error) {
                this.status.textContent = String(error);
            }
        });
        panel.append(choose, save, remove, load, exportSound, el('p', { class: 'hardware-note' }, 'Sounds keep instrument, layers and organic motion. Notes, controllers and tempo stay with the sequence.'));
        this.controls.append(panel);
    }
    private renderLayerOverrides(parent: HTMLElement, layer: Layer) {
        const details = el('details');
        details.append(el('summary', {}, 'Layer sound overrides'));
        for (const [name, key, source, max, step] of [['Tone', 'toneOverride', 'tone', 1, 0.01], ['Attack (s)', 'attackOverride', 'attack', 2, 0.001], ['Release (s)', 'releaseOverride', 'release', 2, 0.001], ['Character', 'characterOverride', 'character', 1, 0.01]] as const) {
            const label = el('label', {}, name), enabled = el('input', { type: 'checkbox' }), field = el('input', { type: 'number', min: '0', max: String(max), step: String(step), 'aria-label': `Layer ${name}` });
            enabled.checked = layer[key] !== undefined && layer[key] !== null;
            enabled.setAttribute('aria-label', `Override layer ${name}`);
            field.value = String(layer[key] ?? this.pattern![source] ?? 0);
            field.disabled = !enabled.checked;
            enabled.onchange = () => { this.snapshot(); field.disabled = !enabled.checked; layer[key] = enabled.checked ? Number(field.value) : undefined; this.changed(); };
            field.onchange = () => {
                const value = Number(field.value);
                if (!Number.isFinite(value) || value < 0 || value > max)
                    return;
                this.snapshot();
                layer[key] = value;
                this.changed();
            };
            label.append(enabled, field);
            details.append(label);
        }
        const seed = el('label', {}, 'Organic seed override'), field = el('input', { 'aria-label': 'Layer organic seed override', inputmode: 'numeric', placeholder: 'Inherit organic seed' });
        field.value = String(layer.organicSeed ?? '');
        field.onchange = () => {
            if (field.value && !/^\d{1,20}$/.test(field.value) || field.value && BigInt(field.value) > 18446744073709551615n) {
                this.status.textContent = 'Layer seed must be an unsigned 64-bit integer.';
                return;
            }
            this.snapshot();
            layer.organicSeed = field.value || undefined;
            this.changed();
        };
        seed.append(field);
        details.append(seed);
        parent.append(details);
    }
    private renderKeyboard() {
        const record = this.button(this.recordingNotes ? '● Recording notes' : 'Record notes', () => {
            this.recordingNotes = !this.recordingNotes;
            if (this.recordingNotes)
                this.recordStarted = performance.now();
            else
                this.activeRecorded.clear();
            record.textContent = this.recordingNotes ? '● Recording notes' : 'Record notes';
            record.setAttribute('aria-pressed', String(this.recordingNotes));
        }, { 'data-operator': '', 'aria-label': 'Record virtual keyboard notes', 'aria-pressed': String(this.recordingNotes) });
        record.disabled = !this.canOperate;
        this.toolbar.insertBefore(record, this.toolbar.lastElementChild);
        const lower = this.button('Octave −', () => { this.releaseNotes(); this.keyboardBase = Math.max(0, this.keyboardBase - 12); this.render(); }), upper = this.button('Octave ＋', () => { this.releaseNotes(); this.keyboardBase = Math.min(108, this.keyboardBase + 12); this.render(); });
        this.toolbar.insertBefore(lower, this.toolbar.lastElementChild);
        this.toolbar.insertBefore(upper, this.toolbar.lastElementChild);
        const keyboard = el('div', { class: 'virtual-keyboard', 'aria-label': 'Server instrument keyboard' });
        const keys = ['a', 'w', 's', 'e', 'd', 'f', 't', 'g', 'y', 'h', 'u', 'j', 'k'];
        for (let i = 0; i < 13; i++) {
            const pitch = this.keyboardBase + i, b = this.button(`${noteName(pitch)}\n${keys[i]!.toUpperCase()}`, () => { }, { 'aria-label': `${noteName(pitch)} keyboard ${keys[i]!.toUpperCase()}`, 'data-pitch': String(pitch), class: [1, 3, 6, 8, 10].includes(i % 12) ? 'black-key' : '' });
            b.disabled = !this.canOperate;
            b.onpointerdown = e => { e.preventDefault(); b.setPointerCapture(e.pointerId); this.pointerNotes.add(pitch); void this.liveNote(pitch, true); };
            const release = () => {
                if (this.pointerNotes.delete(pitch) && !this.focusedKeys.has(pitch))
                    void this.liveNote(pitch, false);
            };
            b.onpointerup = release;
            b.onpointercancel = release;
            b.onlostpointercapture = release;
            bindInstrumentKey(b, () => { this.focusedKeys.add(pitch); void this.liveNote(pitch, true); }, () => { if (this.focusedKeys.delete(pitch) && !this.pointerNotes.has(pitch)) void this.liveNote(pitch, false); });
            keyboard.append(b);
        }
        this.content.append(keyboard);
    }
    private captureNote(pitch: number, velocity: number, on: boolean) {
        const pattern = this.pattern;
        if (!this.recordingNotes || !pattern)
            return;
        if (on)
            this.activeRecorded.set(pitch, { beat: this.previewing ? this.playingPosition : (performance.now() - this.recordStarted) / 1000 * pattern.bpm / 60, time: performance.now(), velocity });
        else {
            const take = this.activeRecorded.get(pitch);
            if (take && take.beat < pattern.beats && pattern.notes.length < 256) {
                this.snapshot();
                const duration = Math.max(0.0625, (performance.now() - take.time) / 1000 * pattern.bpm / 60), startBeat = Math.min(pattern.beats - 0.0625, Math.max(0, Math.round(take.beat * 4) / 4));
                pattern.notes.push({ id: randomUUID().toUpperCase(), pitch, velocity: take.velocity, startBeat, durationBeats: Math.min(pattern.beats - startBeat, duration) });
                this.changed();
            }
            this.activeRecorded.delete(pitch);
        }
    }
    private async liveNote(pitch: number, on: boolean) {
        if (!this.canOperate) {
            this.status.textContent = 'Live keyboard requires an operator role.';
            return;
        }
        const pattern = this.pattern!;
        this.captureNote(pitch, 0.8, on);
        await this.intent(on ? 'designer.noteOn' : 'designer.noteOff', on ? { ...pattern, notes: [], controllers: [] } : undefined, { pitch }, { velocity: 0.8 });
    }
    serverMIDI(event: {
        type: string;
        sources?: {
            id: number;
            name: string;
        }[];
        pitch?: number;
        velocity?: number;
        on?: boolean;
        kind?: Controller['kind'];
        value?: number;
    }) {
        if (event.type === 'midiSources') {
            this.serverMIDISources = event.sources ?? [];
            if (this.dialog.open && this.current === 'designer')
                this.renderMIDISources();
            return;
        }
        if (!this.dialog.open || this.current !== 'designer' || !this.midiSourceID)
            return;
        if (event.type === 'midiNote' && event.pitch !== undefined) {
            for (const key of this.dialog.querySelectorAll<HTMLButtonElement>('.virtual-keyboard button'))
                if (Number(key.dataset.pitch) === event.pitch)
                    key.classList.toggle('sounding', !!event.on);
            this.captureNote(event.pitch, clamp((event.velocity ?? 0) / 127, 0, 1), !!event.on);
            this.status.textContent = `Server MIDI · ${noteName(event.pitch)} · ${event.on ? 'On' : 'Off'}`;
        }
        else if (event.type === 'midiAllNotesOff') {
            this.activeRecorded.clear();
            for (const key of this.dialog.querySelectorAll('.sounding'))
                key.classList.remove('sounding');
        }
        else if (event.type === 'midiController' && event.kind && event.value !== undefined) {
            this.status.textContent = `Server MIDI · ${event.kind} ${event.value.toFixed(2)}`;
            if (this.recordingNotes && this.pattern!.controllers.length < 4096) {
                const beat = Math.round((this.previewing ? this.playingPosition : (performance.now() - this.recordStarted) / 1000 * this.pattern!.bpm / 60) * 16) / 16;
                if (beat <= this.pattern!.beats) {
                    const old = this.pattern!.controllers.find(p => p.kind === event.kind && p.beat === beat);
                    if (old)
                        old.value = event.value;
                    else
                        this.pattern!.controllers.push({ kind: event.kind, beat, value: event.value });
                    this.changed();
                }
            }
        }
    }
    private renderMIDISources() {
        this.controls.querySelector('[data-server-midi]')?.remove();
        const panel = el('details', { 'data-server-midi': '' }), summary = el('summary', {}, 'Server MIDI input');
        panel.append(summary);
        const source = el('select', { 'aria-label': 'Server MIDI source', 'data-operator': '' });
        source.add(new Option('Disconnected', '0'));
        for (const entry of this.serverMIDISources)
            source.add(new Option(entry.name, String(entry.id)));
        source.value = String(this.midiSourceID);
        source.disabled = !this.canOperate;
        source.onchange = () => void this.connectMIDI(Number(source.value));
        const refresh = this.button('Refresh sources', () => void this.intent('designer.midiSources'), { 'data-operator': '' });
        refresh.disabled = !this.canOperate;
        panel.append(source, refresh, el('p', { class: 'hardware-note' }, 'Connected to the hardware server. Record notes captures played notes and controllers.'));
        this.controls.append(panel);
    }
    private async connectMIDI(sourceID: number) {
        if (!this.canOperate)
            return;
        const generation = ++this.midiConnectGeneration;
        this.midiWriter?.cancel();
        if (!sourceID) {
            await this.intent('audition.stop', undefined, {}, {}, { kind: 'designer' });
            if (generation !== this.midiConnectGeneration)
                return;
            this.midiSourceID = 0;
            this.previewing = false;
            this.renderMIDISources();
            return;
        }
        sourceID = Math.trunc(sourceID);
        this.pendingMIDI = this.intent('designer.midiConnect', this.pattern, { sourceID });
        const reply = await this.pendingMIDI;
        this.pendingMIDI = undefined;
        if (generation !== this.midiConnectGeneration)
            return;
        if (reply) {
            this.midiSourceID = sourceID;
            this.midiWriter = new GestureWriter(async (pattern) => !!await this.intent('designer.configure', pattern));
            this.status.textContent = 'Server MIDI connected. Sound edits update the live instrument.';
        }
        else
            this.midiSourceID = 0;
        this.renderMIDISources();
    }
    private releaseNotes() {
        if (this.keys.size || this.pointerNotes.size || this.focusedKeys.size) {
            this.keys.clear();
            this.pointerNotes.clear();
            this.focusedKeys.clear();
            this.activeRecorded.clear();
            void this.intent('designer.allNotesOff');
        }
    }
    private renderControllers() {
        const pattern = this.pattern!, panel = el('details');
        panel.append(el('summary', {}, 'Performance controllers'), this.button('Open controller chart', () => { this.releaseNotes(); this.controllerEditor.open(); }));
        const kind = el('select', { 'aria-label': 'Controller kind' });
        for (const c of ['sustain', 'modulation', 'expression', 'pitchBend'])
            kind.add(new Option(c, c));
        const beat = el('input', { type: 'number', min: '0', max: String(pattern.beats), step: '0.25', value: '0', 'aria-label': 'Controller beat' }), value = el('input', { type: 'number', min: '0', max: '1', step: '0.01', value: '0', 'aria-label': 'Controller value' });
        panel.append(kind, beat, value, this.button('Add point', () => {
            const b = Number(beat.value), v = Number(value.value);
            if (!Number.isFinite(b) || !Number.isFinite(v) || b < 0 || b > pattern.beats || v < 0 || v > 1 || pattern.controllers.length >= 4096) {
                this.status.textContent = 'Choose a controller position in the pattern and a value from 0–1.';
                return;
            }
            this.snapshot();
            pattern.controllers.push({ kind: kind.value as Controller['kind'], beat: b, value: v });
            pattern.controllers.sort((a, b) => a.beat - b.beat);
            this.render();
            this.changed();
        }));
        for (const p of pattern.controllers.slice(0, 128)) {
            const row = el('div', { class: 'controller-row' });
            row.append(el('span', {}, `${p.kind} · beat ${p.beat.toFixed(2)} · ${p.value.toFixed(2)}`), this.button('×', () => { this.snapshot(); pattern.controllers = pattern.controllers.filter(c => c !== p); this.render(); this.changed(); }, { 'aria-label': 'Remove controller point' }));
            panel.append(row);
        }
        if (pattern.controllers.length > 128)
            panel.append(el('p', { class: 'hardware-note' }, `${pattern.controllers.length} points retained; first 128 shown.`));
        this.controls.append(panel);
    }
    private noteDown(e: PointerEvent) {
        if (!this.enabled)
            return;
        const pattern = this.pattern!, canvas = this.canvas!, row = 18, x = e.offsetX - 54 + this.offset, pitch = clamp(this.topPitch - Math.floor((e.offsetY - 26) / row), 0, 127), beat = Math.max(0, Math.round(x / this.beatPixels * 4) / 4);
        if (e.offsetY < 26 || e.offsetX < 54)
            return;
        const note = pattern.notes.find(n => n.pitch === pitch && beat >= n.startBeat && beat < n.startBeat + n.durationBeats);
        if (!e.shiftKey)
            this.selected.clear();
        if (note) {
            this.selected.add(note.id);
            this.snapshot();
            this.noteGestureOriginal = structuredClone(pattern);
            this.noteDrag = { id: note.id, original: { ...note }, x: e.offsetX, y: e.offsetY, resize: Math.abs((note.startBeat + note.durationBeats) * this.beatPixels - this.offset + 54 - e.offsetX) < 9 };
        }
        else if (beat < pattern.beats) {
            if (pattern.notes.length >= 256) {
                this.status.textContent = 'A pattern supports 256 notes. Choose a shorter MIDI excerpt.';
                return;
            }
            this.snapshot();
            const n: Note = { id: randomUUID().toUpperCase(), pitch, velocity: 0.8, startBeat: beat, durationBeats: Math.min(0.5, pattern.beats - beat) };
            if (pattern.notes.length >= 256) {
                this.status.textContent = 'A pattern supports 256 notes. Shorten the selected MIDI excerpt.';
                return;
            }
            pattern.notes.push(n);
            this.selected.add(n.id);
            this.changed();
        }
        canvas.setPointerCapture(e.pointerId);
        canvas.focus();
        this.dirty = true;
        this.renderNoteProperties();
    }
    private noteMove(e: PointerEvent) {
        const d = this.noteDrag, n = this.pattern?.notes.find(n => n.id === d?.id);
        if (!d || !n)
            return;
        const delta = Math.round((e.offsetX - d.x) / this.beatPixels * 4) / 4;
        if (d.resize)
            n.durationBeats = clamp(d.original.durationBeats + delta, 0.25, this.pattern!.beats - n.startBeat);
        else {
            n.startBeat = clamp(d.original.startBeat + delta, 0, this.pattern!.beats - n.durationBeats);
            n.pitch = clamp(d.original.pitch - Math.round((e.offsetY - d.y) / 18), 0, 127);
        }
        this.changed();
    }
    private renderNoteProperties() {
        this.controls.querySelector('[data-note]')?.remove();
        const note = this.pattern?.notes.find(n => this.selected.has(n.id));
        if (!note)
            return;
        const panel = el('details', { open: '', 'data-note': '' });
        panel.append(el('summary', {}, `${noteName(note.pitch)} · selected note`));
        for (const [name, key, min, max, step] of [['Pitch', 'pitch', 0, 127, 1], ['Start beat', 'startBeat', 0, this.pattern!.beats, 0.25], ['Duration', 'durationBeats', 0.0625, this.pattern!.beats, 0.0625], ['Velocity', 'velocity', 0, 1, 0.01]] as const)
            this.numeric(panel, name, key, min, max, step, note as unknown as Record<string, unknown>);
        const articulation = el('select', { 'aria-label': 'Note articulation' });
        for (const a of ['normal', 'staccato', 'tenuto', 'accent'])
            articulation.add(new Option(a, a));
        articulation.value = note.articulation ?? 'normal';
        articulation.onchange = () => { this.snapshot(); note.articulation = articulation.value; this.changed(); };
        panel.append(articulation);
        this.controls.append(panel);
    }
    private localUndo(redo: boolean) {
        const source = redo ? this.redo : this.undo, target = redo ? this.undo : this.redo, previous = source.pop();
        const current = this.current === 'designer' ? this.pattern : this.beatDocument();
        if (previous && current) {
            target.push(structuredClone(current));
            if (this.current === 'designer')
                this.pattern = previous as Pattern;
            else
                this.installBeatDocument(previous as DrumDocument);
            this.render();
            this.changed();
        }
    }
    private keyDown(e: KeyboardEvent) {
        if (!this.enabled)
            return;
        if (!acceptWorkspaceShortcut(e))
            return;
        if (e.code === 'Space') {
            e.preventDefault();
            e.stopPropagation();
            void this.togglePreview();
            return;
        }
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'z') {
            e.preventDefault();
            e.stopPropagation();
            this.localUndo(e.shiftKey);
            return;
        }
        if (this.current !== 'designer')
            return;
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'a') { e.preventDefault(); e.stopPropagation(); this.selected=new Set(this.pattern!.notes.map(n=>n.id));this.dirty=true;this.status.textContent=`Selected ${this.selected.size} notes in this Designer draft.`;return; }
        if(isItemNavigation(e.key) && !e.metaKey && !e.ctrlKey && !e.altKey) { e.preventDefault();e.stopPropagation();const selected=this.pattern!.notes.find(n=>this.selected.has(n.id)),note=navigateTimelineItem(this.pattern!.notes,selected,e.key,n=>n.startBeat);if(note){this.selected=new Set([note.id]);this.revealNote(note);this.dirty=true;this.status.textContent=`Selected ${noteName(note.pitch)} at beat ${(note.startBeat+1).toFixed(3)}, velocity ${Math.round(note.velocity*127)}. Arrow keys move; Shift arrows resize.`;}return; }
        const keys = ['a', 'w', 's', 'e', 'd', 'f', 't', 'g', 'y', 'h', 'u', 'j', 'k'], index = keys.indexOf(e.key.toLowerCase());
        if (index >= 0 && !e.metaKey && !e.ctrlKey && !e.altKey && !e.repeat) {
            e.preventDefault();
            this.keys.set(e.code, this.keyboardBase + index);
            void this.liveNote(this.keyboardBase + index, true);
            return;
        }
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'z') {
            e.preventDefault();
            e.stopPropagation();
            this.localUndo(e.shiftKey);
            return;
        }
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'd') {
            e.preventDefault();
            this.snapshot();
            const notes = this.pattern!.notes.filter(n => this.selected.has(n.id));
            if (this.pattern!.notes.length + notes.length > 256)
                return;
            this.selected.clear();
            for (const n of notes) {
                const copy = { ...n, id: randomUUID().toUpperCase(), startBeat: Math.min(this.pattern!.beats - n.durationBeats, n.startBeat + 0.5) };
                this.pattern!.notes.push(copy);
                this.selected.add(copy.id);
            }
            this.changed();
            return;
        }
        if (['Delete', 'Backspace'].includes(e.key)) {
            e.preventDefault();
            e.stopPropagation();
            this.snapshot();
            this.pattern!.notes = this.pattern!.notes.filter(n => !this.selected.has(n.id));
            this.selected.clear();
            this.changed();
            return;
        }
        if (e.key.startsWith('Arrow') && this.selected.size) {
            e.preventDefault();
            this.snapshot();
            for (const n of this.pattern!.notes.filter(n => this.selected.has(n.id))) {
                if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
                    const delta = e.key === 'ArrowLeft' ? -0.25 : 0.25;
                    if (e.shiftKey)
                        n.durationBeats = clamp(n.durationBeats + delta, 0.25, this.pattern!.beats - n.startBeat);
                    else
                        n.startBeat = clamp(n.startBeat + delta, 0, this.pattern!.beats - n.durationBeats);
                }
                else
                    n.pitch = clamp(n.pitch + (e.key === 'ArrowUp' ? 1 : -1), 0, 127);
            }
            this.changed();
        }
    }
    private revealNote(note: Note) {
        const width=this.canvas?.clientWidth??800,height=this.canvas?.clientHeight??300,x=54+note.startBeat*this.beatPixels-this.offset,y=26+(this.topPitch-note.pitch)*18;
        if(x<70||x>width-25)this.offset=Math.max(0,note.startBeat*this.beatPixels-(width-54)/2);
        if(y<40)this.topPitch=Math.min(127,note.pitch+2);else if(y>height-25)this.topPitch=Math.min(127,note.pitch+Math.floor((height-50)/18));
    }
    private keyUp(e: KeyboardEvent) {
        const pitch = this.keys.get(e.code);
        if (pitch !== undefined) {
            this.keys.delete(e.code);
            void this.liveNote(pitch, false);
        }
    }
    private tick() {
        if (this.dialog.open && this.current === 'designer' && (this.dirty || this.previewing)) {
            this.drawNotes();
            this.dirty = false;
        }
        this.raf = requestAnimationFrame(() => this.tick());
    }
    private drawNotes() {
        const canvas = this.canvas, pattern = this.pattern;
        if (!canvas || !pattern)
            return;
        const c = canvas.getContext('2d')!, w = canvas.clientWidth, h = canvas.clientHeight;
        c.fillStyle = '#131a22';
        c.fillRect(0, 0, w, h);
        c.font = '10px system-ui';
        for (let row = 0; row < Math.ceil((h - 26) / 18); row++) {
            const pitch = this.topPitch - row, y = 26 + row * 18;
            if (pitch < 0 || pitch > 127)
                continue;
            c.fillStyle = [1, 3, 6, 8, 10].includes(pitch % 12) ? '#17212c' : '#1c2834';
            c.fillRect(54, y, w - 54, 17);
            c.fillStyle = '#a8b8c8';
            c.fillText(noteName(pitch), 6, y + 13);
        }
        const first = Math.floor(this.offset / this.beatPixels), last = Math.ceil((this.offset + w - 54) / this.beatPixels);
        c.fillStyle = '#a8b8c8';
        for (let beat = first; beat <= last; beat++) {
            const x = 54 + beat * this.beatPixels - this.offset;
            c.strokeStyle = beat % 4 === 0 ? '#556b7d' : '#344454';
            c.beginPath();
            c.moveTo(x, 26);
            c.lineTo(x, h);
            c.stroke();
            c.fillText(String(beat + 1), x + 5, 17);
        }
        for (const n of pattern.notes) {
            const x = 54 + n.startBeat * this.beatPixels - this.offset, y = 26 + (this.topPitch - n.pitch) * 18, width = n.durationBeats * this.beatPixels;
            if (x + width < 54 || x > w || y < 26 || y > h)
                continue;
            c.fillStyle = this.selected.has(n.id) ? '#a6d1ec' : '#779eb5';
            c.fillRect(Math.max(54, x), y + 2, Math.max(2, Math.min(w, x + width) - Math.max(54, x) - 1), 14);
            c.fillStyle = '#182b39';
            c.fillRect(Math.max(54, x), y + 13, Math.max(2, (width - 1) * n.velocity), 3);
        }
        if (this.previewing) {
            const beat = this.playingPosition, x = 54 + beat * this.beatPixels - this.offset;
            c.strokeStyle = '#ecdf9f';
            c.beginPath();
            c.moveTo(x, 26);
            c.lineTo(x, h);
            c.stroke();
        }
    }
    private importMIDI() {
        const file = el('input', { type: 'file', accept: '.mid,.midi' });
        file.onchange = () => {
            const selected = file.files?.[0];
            if (!selected)
                return;
            if (selected.size > 4194304) {
                this.status.textContent = 'MIDI is limited to 4 MiB.';
                return;
            }
            this.status.textContent = 'Inspecting MIDI…';
            this.midiWorker?.terminate();
            this.midiWorker = new Worker(new URL('./midi-worker.js', import.meta.url), { type: 'module' });
            this.midiWorker.onmessage = e => {
                if (e.data.error) {
                    this.status.textContent = e.data.error;
                    return;
                }
                this.midi = e.data.content;
                this.showMIDIExcerpt();
            };
            void selected.arrayBuffer().then(buffer => this.midiWorker!.postMessage({ id: randomUUID(), buffer }, [buffer]));
        };
        file.click();
    }
    private showMIDIExcerpt() {
        const content = this.midi!, panel = el('details', { open: '', class: 'midi-excerpt' });
        panel.append(el('summary', {}, `MIDI · ${content.beats.toFixed(1)} beats · ${content.bpm.toFixed(1)} BPM`));
        const part = el('select', { 'aria-label': 'MIDI part' });
        for (const p of content.parts)
            part.add(new Option(`${p.name} · ${p.notes.length} notes`, String(p.id)));
        const start = el('input', { type: 'number', min: '0', step: '0.25', value: '0', 'aria-label': 'MIDI excerpt start beat' }), length = el('input', { type: 'number', min: '0.25', max: '64', step: '0.25', value: String(Math.min(64, content.beats)), 'aria-label': 'MIDI excerpt length beats' });
        const apply = this.button('Import excerpt', () => {
            try {
                const excerpt = MIDIExcerpt(content, Number(part.value), Number(start.value), Number(length.value));
                this.snapshot();
                Object.assign(this.pattern!, excerpt);
                this.render();
                this.changed();
                this.status.textContent = `Imported ${excerpt.notes.length} notes from ${part.selectedOptions[0]!.textContent}.`;
            }
            catch (error) {
                this.status.textContent = error instanceof Error ? error.message : 'MIDI import failed.';
            }
        });
        panel.append(part, start, length, apply);
        this.controls.prepend(panel);
    }
    private beatDocument(): DrumDocument { const document = this.beatBank ?? { version: 2, patterns: [], selected: 0 }; document.patterns[document.selected] = this.drums as unknown as WireDrumPattern; return document; }
    private installBeatDocument(document: DrumDocument) { this.beatBank = document; this.drums = document.patterns[document.selected] as unknown as DrumPattern; this.selectedStep = undefined; }
    private paintBeatButton(button: HTMLButtonElement, voice: string, index: number) { const step = this.drums?.steps.find(s => s.voice === voice && s.index === index); button.setAttribute('aria-pressed', String(!!step)); button.style.opacity = step ? String(0.35 + step.velocity * 0.65) : '1'; }
    private renderBeatBank() { this.controls.querySelector('[data-beat-bank]')?.remove(); const panel = el('details', { 'data-beat-bank': '' }), document = this.beatDocument(); panel.append(el('summary', {}, 'Patterns')); const select = el('select', { 'aria-label': 'Pattern bank' }); document.patterns.forEach((p, i) => select.add(new Option(p.name, String(i)))); select.value = String(document.selected); select.onchange = () => { document.selected = Number(select.value); this.installBeatDocument(document); this.render(); this.changed(); }; panel.append(select); const name = el('input', { 'aria-label': 'Pattern name', maxlength: '120' }); name.value = this.drums!.name; name.onchange = () => { if (!name.value.trim())
        return; this.snapshot(); this.drums!.name = name.value.trim(); this.renderBeatBank(); this.changed(); }; panel.append(name); for (const [label, empty] of [['New', true], ['Duplicate', false]] as const) {
        const button = this.button(label, () => { try {
            this.snapshot();
            this.installBeatDocument(duplicateDrumPattern(this.beatDocument(), empty));
            this.render();
            this.changed();
        }
        catch (error) {
            this.status.textContent = String(error);
        } });
        button.disabled = document.patterns.length >= 32;
        panel.append(button);
    } panel.append(this.button('Copy pattern', () => { this.beatClipboard = structuredClone(this.drums!); this.status.textContent = 'Pattern copied in this client.'; }), this.button('Paste pattern', () => { if (!this.beatClipboard) {
        this.status.textContent = 'Copy a pattern first.';
        return;
    } this.snapshot(); this.drums = structuredClone(this.beatClipboard); this.render(); this.changed(); }), this.button('Clear hits', () => { this.snapshot(); this.drums!.steps = []; this.selectedStep = undefined; this.renderBeatsGrid(); this.changed(); }), this.button('Save patterns', () => { try {
        const d = this.beatDocument();
        d.patterns.forEach(validateDrumPattern);
        const url = URL.createObjectURL(new Blob([JSON.stringify(d, null, 2)], { type: 'application/json' })), link = el('a');
        link.href = url;
        link.download = this.drums!.name.replace(/[\/:*?"<>|]/g, '-') + '.soundroomdrums';
        link.click();
        setTimeout(() => URL.revokeObjectURL(url), 10000);
    }
    catch (error) {
        this.status.textContent = String(error);
    } }), this.button('Open patterns', () => { const file = el('input', { type: 'file', accept: '.soundroomdrums,.json' }); file.onchange = () => { const selected = file.files?.[0]; if (!selected)
        return; if (selected.size > 2000000) {
        this.status.textContent = 'Drum documents are limited to 2 MB.';
        return;
    } void selected.text().then(text => { const incoming = parseDrumDocument(text); this.snapshot(); this.installBeatDocument(incoming); this.render(); this.changed(); }).catch(error => this.status.textContent = String(error)); }; file.click(); })); this.controls.append(panel); }
    private renderBeats() {
        const pattern = this.drums!, kit = el('select', { 'aria-label': 'Drum kit' });
        for (const k of this.kits)
            kit.add(new Option(k.name, k.id));
        kit.value = pattern.kit;
        kit.onchange = () => { this.snapshot(); pattern.kit = kit.value; this.render(); this.changed(); };
        this.toolbar.insertBefore(kit, this.toolbar.lastElementChild);
        const preset = el('select', { 'aria-label': 'Drum pattern preset' });
        preset.add(new Option('Choose pattern…', ''));
        for (const p of this.drumPresets)
            preset.add(new Option(p.name, p.id));
        preset.onchange = () => {
            const p = this.drumPresets.find(p => p.id === preset.value);
            if (p) {
                this.snapshot();
                const currentKit = pattern.kit;
                this.drums = { ...structuredClone(p.pattern), kit: currentKit, bpm: pattern.bpm };
                this.render();
                this.changed();
            }
        };
        this.toolbar.insertBefore(preset, this.toolbar.lastElementChild);
        for (const [name, key, min, max, step] of [['BPM', 'bpm', 30, 300, 0.1], ['Bars', 'bars', 1, 8, 1], ['Swing', 'swing', 0, 0.7, 0.01], ['Timing humanize (s)', 'humanize', 0, 0.025, 0.001], ['Velocity variation', 'velocityHumanize', 0, 0.2, 0.01], ['Master gain', 'masterGain', 0, 1, 0.01]] as const)
            this.numeric(this.controls, name, key, min, max, step, pattern as unknown as Record<string, unknown>, () => { if (key === 'bars') {
                this.drums = resizeDrumBars(pattern as unknown as WireDrumPattern, pattern.bars) as unknown as DrumPattern;
                this.selectedStep = undefined;
                this.render();
            } this.changed(); });
        const division = el('select', { 'aria-label': 'Drum steps per beat' });
        for (const value of [2, 4, 8])
            division.add(new Option(String(value), String(value)));
        division.value = String(pattern.stepsPerBeat);
        division.onchange = () => { this.snapshot(); this.drums = remapDrumGrid(pattern as unknown as WireDrumPattern, Number(division.value) as 2 | 4 | 8) as unknown as DrumPattern; this.selectedStep = undefined; this.render(); this.changed(); };
        this.controls.append(el('label', {}, 'Steps per beat'), division);
        const cycles = el('select', { 'aria-label': 'Variation cycles' });
        for (const v of [1, 2, 4, 8])
            cycles.add(new Option(String(v), String(v)));
        cycles.value = String(pattern.variationCycles ?? 1);
        cycles.onchange = () => { this.snapshot(); pattern.variationCycles = Number(cycles.value); this.changed(); };
        this.controls.append(el('label', {}, 'Variation cycles'), cycles);
        this.renderBeatBank();
        this.renderBeatsGrid();
    }
    private renderBeatsGrid() {
        const pattern = this.drums!, kit = this.kits.find(k => k.id === pattern.kit)!, count = pattern.bars * 4 * (pattern.stepsPerBeat === 2 ? 2 : pattern.stepsPerBeat === 8 ? 8 : 4);
        this.content.replaceChildren();
        const grid = el('div', { class: 'beat-grid', role: 'grid', 'aria-label': 'Drum step sequencer' });
        for (const voice of kit.voices) {
            const row = el('div', { class: 'beat-row', role: 'row' }), name = el('button', { 'aria-label': `Audition ${voice}` }, voice.replace(/([A-Z])/g, ' $1'));
            name.onclick = () => this.renderLaneProperties(voice);
            row.append(name);
            const hit = this.button('▶', () => void this.intent('drums.preview', pattern, {}, {}, { audition: voice }), { 'aria-label': `Audition ${voice}` });
            hit.disabled = !this.canOperate;
            row.append(hit);
            for (let index = 0; index < count; index++) {
                const step = pattern.steps.find(s => s.voice === voice && s.index === index), button = this.button(String(index + 1), () => {
                    this.snapshot();
                    const old = pattern.steps.find(s => s.voice === voice && s.index === index);
                    if (old) {
                        pattern.steps = pattern.steps.filter(s => s !== old);
                        this.selectedStep = undefined;
                    }
                    else {
                        this.selectedStep = { voice, index, velocity: 0.8, pan: 0, probability: 1 };
                        pattern.steps.push(this.selectedStep);
                    }
                    this.paintBeatButton(button, voice, index);
                    this.renderStepProperties();
                    this.changed();
                }, { role: 'gridcell', 'aria-label': `${voice} step ${index + 1}`, 'aria-pressed': String(!!step), class: index % pattern.stepsPerBeat === 0 ? 'on-beat' : '' });
                button.dataset.voice = voice;
                button.dataset.step = String(index);
                button.oncontextmenu = e => {
                    e.preventDefault();
                    const current = pattern.steps.find(s => s.voice === voice && s.index === index);
                    if (current) {
                        this.selectedStep = current;
                        this.renderStepProperties();
                    }
                };
                if (step)
                    button.style.opacity = String(0.35 + step.velocity * 0.65);
                row.append(button);
            }
            grid.append(row);
        }
        this.content.append(grid);
        this.content.append(el('p', { class: 'hardware-note' }, 'Click to toggle a hit. Right-click an active hit for velocity, probability, pan and articulation.'));
    }
    private renderLaneProperties(voice: string) {
        this.controls.querySelector('[data-lane]')?.remove();
        const pattern = this.drums!, lane = pattern.lanes[voice] ?? { kit: pattern.kit, gain: 1, pan: 0, tuning: 0, muted: false };
        const panel = el('details', { open: '', 'data-lane': '' });
        panel.append(el('summary', {}, `${voice} · instrument lane`));
        const kit = el('select', { 'aria-label': `${voice} kit` });
        for (const k of this.kits)
            kit.add(new Option(k.name, k.id));
        kit.value = lane.kit;
        kit.onchange = () => { this.snapshot(); lane.kit = kit.value; pattern.lanes[voice] = lane; this.changed(); };
        panel.append(kit);
        for (const [name, key, min, max, step] of [['Gain', 'gain', 0, 1, 0.01], ['Pan', 'pan', -1, 1, 0.01], ['Tuning (semitones)', 'tuning', -12, 12, 0.1], ['Damping', 'damping', 0, 1, 0.01]] as const)
            this.numeric(panel, name, key, min, max, step, lane as unknown as Record<string, unknown>, () => { pattern.lanes[voice] = lane; this.changed(); });
        const muted = this.button(lane.muted ? 'Unmute lane' : 'Mute lane', () => { this.snapshot(); lane.muted = !lane.muted; pattern.lanes[voice] = lane; muted.textContent = lane.muted ? 'Unmute lane' : 'Mute lane'; this.changed(); });
        panel.append(muted);
        this.controls.append(panel);
    }
    private renderStepProperties() {
        this.controls.querySelector('[data-step]')?.remove();
        const step = this.selectedStep;
        if (!step)
            return;
        const panel = el('details', { open: '', 'data-step': '' });
        panel.append(el('summary', {}, `${step.voice} · step ${step.index + 1}`));
        for (const [name, key, min, max, inc] of [['Velocity', 'velocity', 0, 1, 0.01], ['Probability', 'probability', 0, 1, 0.01], ['Pan', 'pan', -1, 1, 0.01]] as const)
            this.numeric(panel, name, key, min, max, inc, step as unknown as Record<string, unknown>, () => { for (const button of this.content.querySelectorAll<HTMLButtonElement>('[data-step]'))
                if (button.dataset.voice === step.voice && Number(button.dataset.step) === step.index)
                    this.paintBeatButton(button, step.voice, step.index); this.changed(); });
        const articulation = el('select', { 'aria-label': 'Drum articulation' });
        for (const a of this.kits.find(k => k.id === this.drums!.kit)?.articulations[step.voice] ?? ['center'])
            articulation.add(new Option(a, a));
        articulation.value = step.articulation ?? 'center';
        articulation.onchange = () => { this.snapshot(); step.articulation = articulation.value; this.changed(); };
        panel.append(articulation);
        this.controls.append(panel);
    }
}
