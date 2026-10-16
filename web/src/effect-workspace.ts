import {randomUUID} from './crypto.js';
import { ownAcceptedTrackRevision } from './gesture-revision.js';
import { acceptWorkspaceShortcut, openWorkspaceDialog } from './input-focus.js';
import { EffectLibrary, applyEffectPreset, presetFromEffect, prepareEffectChain, type SavedEffectPreset, type SavedEffectChain } from './effect-presets.js';
import { PresetManager } from './preset-manager.js';
import type { Command, SessionMirror, Track } from './model.js';
import type { RealtimeEffect, EffectMeterSnapshot } from './generated/models.js';
import { type EffectMetadata, type Parameter, trackControls, parameterValue, parameterNormalized } from './effects.js';
import { effectIdentity, effectGroups, parameterDisplay, inputScale, editableEffect, sameEffectSettings, eqResponse, sectionResponse, compressorOutput, warmthCurve } from './effect-models.js';
import {AutomationTouchBindings,TouchControlBinding,type TouchGesture} from './automation-touch.js';
import { GestureWriter } from './automation.js';
import { encodeBase64 } from './sdk.js';
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, text?: string) => { const e = document.createElement(tag); if (text)
    e.textContent = text; return e; };
type Sender = (command: Command, trackID?: string, expectedTrackRevision?:number) => Promise<{
    revision?:number;
    payload?: unknown;
} | undefined>;
export class EffectWorkspace {
    private dialog: HTMLDialogElement;
    private toolbar: HTMLElement;
    private controls: HTMLElement;
    private graph: HTMLCanvasElement;
    private caption: HTMLElement;
    private metersPanel: HTMLElement;
    private status: HTMLElement;
    private context: CanvasRenderingContext2D;
    private effectID = '';
    private trackID = '';
    private draft?: RealtimeEffect;
    private metadata?: EffectMetadata;
    private meter?: EffectMeterSnapshot;
    private meteredAt = 0;
    private width = 800;
    private height = 260;
    private dirty = true;
    private writer?: GestureWriter<{
        effect: RealtimeEffect;
        name: string;
        gesture: string;
    }>;
    private gestureName = 'Adjust effect';
    private staleMeters = false;
    private gesture = '';
    private writing = 0;
    private trackRevision = 0;
    private generation=0;
    private sourceEpoch?:number;
    private blocked=false;
    private graphDrag?: string;
    private updates = new Map<string, (value: number) => void>();
    private manager?: PresetManager;
    private managerGeneration=-1;
    constructor(private mirror: SessionMirror, private send: Sender, private effects: () => EffectMetadata[], private enabled: () => boolean, private canOperate: () => boolean, private openPitch: (effectID: string,trackID?:string) => void, private automate: (trackID: string, effectID: string, parameter: string) => void, private library?: EffectLibrary, private epoch?:()=>number, private touch?:AutomationTouchBindings) {
        if (library) {
            this.manager = new PresetManager(library, this.effects, () => this.managerGeneration===this.generation?this.current():undefined, () => this.managerGeneration===this.generation?this.track()?.realtimeEffects as RealtimeEffect[]??[]:[], () => this.enabled()&&this.ready()&&this.managerGeneration===this.generation, preset => this.applySaved(preset), chain => this.applyChain(chain));
            library.addEventListener('change', () => { if (this.dialog.open) this.render(); });
        }
        this.dialog = el('dialog');
        this.dialog.className = 'effect-dialog';
        this.dialog.setAttribute('aria-label','Precision effect controls');
        this.toolbar = el('div');
        this.toolbar.className = 'workspace-toolbar';
        this.controls = el('div');
        this.controls.className = 'full-effect-controls';
        this.graph = el('canvas');
        this.graph.className = 'effect-graph';
        this.graph.tabIndex = 0;
        this.graph.setAttribute('role', 'img');
        this.caption = el('p');
        this.caption.className = 'graph-caption';
        this.metersPanel = el('div');
        this.metersPanel.className = 'effect-meters';
        this.status = el('div');
        this.status.className = 'workspace-status';
        this.status.setAttribute('role', 'status');
        const c = this.graph.getContext('2d');
        if (!c)
            throw Error('Effect graph unavailable.');
        this.context = c;
        this.dialog.append(this.toolbar, this.graph, this.caption, this.metersPanel, this.controls, this.status);
        document.body.append(this.dialog);
        this.dialog.oncancel = e => { e.preventDefault(); void this.close(); };
        new ResizeObserver(() => { this.width = this.graph.clientWidth; const dpr = Math.min(devicePixelRatio, 2); this.graph.width = this.width * dpr; this.graph.height = this.height * dpr; this.context.setTransform(dpr, 0, 0, dpr, 0, 0); this.dirty = true; }).observe(this.graph);
        this.graph.onpointerdown = e => this.down(e);
        this.graph.onpointermove = e => this.move(e);
        this.graph.onpointerup = e => { this.graphDrag = undefined; if (this.graph.hasPointerCapture(e.pointerId))
            this.graph.releasePointerCapture(e.pointerId); };
        this.graph.onpointercancel = () => { this.writer?.cancel(); this.graphDrag = undefined; };
        this.dialog.addEventListener('keydown', e => { if (!acceptWorkspaceShortcut(e))
            return; if (e.code === 'Space') {
            e.preventDefault();
            if (this.canOperate())
                void this.transport();
        } });
        requestAnimationFrame(() => this.tick());
    }
    private track() { const s = this.mirror.snapshot; return this.trackID === s?.session.masterTrack.id ? s.session.masterTrack : s?.session.tracks.find(t => t.id === this.trackID); }
    private current() { return this.track()?.realtimeEffects.find(e => (e as RealtimeEffect).id === this.effectID) as RealtimeEffect | undefined; }
    open(trackID: string, effectID: string) { const snapshot = this.mirror.snapshot, track = trackID === snapshot?.session.masterTrack.id ? snapshot.session.masterTrack : snapshot?.session.tracks.find(t => t.id === trackID), effect = track?.realtimeEffects.find(e => (e as RealtimeEffect).id === effectID) as RealtimeEffect | undefined, metadata = this.effects().find(m => m.id === effect?.kind); if (!track || !effect || !metadata)
        return; const generation=++this.generation; this.sourceEpoch=this.epoch?.(); this.blocked=false; this.graphDrag=undefined; this.trackID = trackID; this.effectID = effectID; this.metadata = metadata; this.draft = editableEffect(effect, metadata); this.trackRevision = snapshot!.trackRevisions[trackID] ?? 0; this.meter = undefined; this.meteredAt = 0; this.writing = 0; this.writer?.cancel(); this.writer = new GestureWriter(async (item) => generation===this.generation&&await this.commit(item.effect, item.name, item.gesture)); this.status.textContent = ''; this.render(); openWorkspaceDialog(this.dialog,this.graph); this.dirty = true; void this.subscribe([effectID]); }
    private ready() {return !this.blocked&&this.sourceEpoch===this.epoch?.()&&!!this.current()&&this.mirror.snapshot?.trackRevisions[this.trackID]===this.trackRevision;}
    private conflict(message='This track changed or the edit could not be confirmed. Your settings are retained here; reopen the effect to review the server version.') {this.blocked=true;this.status.textContent=message;}
    private async subscribe(ids: string[]) { await this.send({ type: 'editor', intent: { operation: 'meters.subscribe', context: { cursorFrame: 0, clipIDs: [] }, integers: {}, numbers: {}, strings: {}, flags: {}, payload: encodeBase64(new TextEncoder().encode(JSON.stringify(ids))) } }); }
    readings(values: Record<string, EffectMeterSnapshot>) { if (!this.dialog.open)
        return; const meter = values[this.effectID]; if (meter) {
        this.meter = meter;
        this.meteredAt = performance.now();
        this.renderMeters();
        if (this.draft?.kind === 'resonance')
            this.dirty = true;
    } }
    private button(label: string, action: () => void, operate = false) { const b = el('button', label); b.disabled = !this.enabled() || operate && !this.canOperate(); b.onclick = action; return b; }
    private render() { const metadata = this.metadata!, effect = this.draft!, identity = effectIdentity[effect.kind] ?? { icon: '◈', color: '#9eb7cc' }; this.dialog.style.setProperty('--effect-color', identity.color); this.toolbar.replaceChildren(); this.toolbar.append(el('strong', `${identity.icon} ${metadata.name} · ${this.track()?.name ?? 'Track'}`)); const enabled = el('label', 'Enabled'), toggle = el('input'); toggle.type = 'checkbox'; toggle.checked = effect.enabled ?? true; toggle.disabled = !this.enabled(); toggle.onchange = () => this.change({ enabled: toggle.checked }, 'Toggle effect'); enabled.append(toggle); const playTrack = this.button('Play track', () => void this.transport(true), true); playTrack.disabled ||= !this.track()?.clips.length; this.toolbar.append(enabled, playTrack, this.button('Play mix', () => void this.transport(false), true)); const preset = el('select'); preset.setAttribute('aria-label', `${metadata.name} preset`); preset.add(new Option('Preset…', '')); const presets = [...(metadata.factoryPresets ?? []), ...this.customPresets(metadata.id).map(p => ({ ...p, name: p.name + ' · custom' }))]; for (const [index, p] of presets.entries())
        preset.add(new Option(p.name, String(index))); const native = this.library?.presets(metadata.id) ?? []; for (const p of native) preset.add(new Option(`${p.name} · saved${p.legacyAmount != null ? ' · legacy' : ''}`, `native:${p.id}`)); preset.disabled = !this.enabled(); preset.onchange = () => { if(!preset.value)return; if (preset.value.startsWith('native:')) { const selected=native.find(p=>p.id===preset.value.slice(7)); if(selected)void this.applySaved(selected); return; } const selected = presets[Number(preset.value)]; if (selected) {
        this.change({ parameters: { ...this.draft!.parameters, ...selected.values } }, `${metadata.name} preset`);
        this.renderControls();
    } }; this.toolbar.append(preset, this.button('Save preset', () => { void this.savePreset(); })); if(this.manager) this.toolbar.append(this.button('Manage saved', () => { void this.openManager(); })); if (effect.kind === 'pitchCorrection')
        this.toolbar.append(this.button('Pitch chart', () => { void this.openPitchChart(); })); const apply = this.button('Apply to audio', () => void this.bake()); apply.disabled = !this.enabled() || !this.track()?.clips.length || !this.ready() || this.writing>0; this.toolbar.append(apply); const close = el('button', 'Done'); close.onclick = () => void this.close(); this.toolbar.append(close); this.renderControls(); this.renderMeters(); this.dirty = true; }
    private renderControls() { const metadata = this.metadata!, effect = this.draft!; this.controls.replaceChildren(); this.updates.clear(); const groups = effectGroups[effect.kind] ?? [{ name: 'Controls', keys: metadata.parameters.map(p => p.id) }], used = new Set(groups.flatMap(g => g.keys)), all = [...groups, ...(metadata.parameters.some(p => !used.has(p.id)) ? [{ name: 'More controls', keys: metadata.parameters.filter(p => !used.has(p.id)).map(p => p.id) }] : [])]; for (const group of all) {
        const section = el('section');
        section.append(el('h3', group.name));
        for (const key of group.keys) {
            const parameter = metadata.parameters.find(p => p.id === key);
            if (parameter)
                section.append(this.parameter(parameter));
        }
        this.controls.append(section);
    } }
    private parameter(p:Parameter) {
        const generation=this.generation,trackID=this.trackID,effectID=this.effectID,target={kind:'effectParameter' as const,effectID,parameterID:p.id};
        const row=el('div');row.className='full-parameter';const label=el('label',p.name),field=p.choices?el('select'):el('input'),value=this.draft!.parameters?.[p.id]??p.defaultValue,scale=inputScale(p);
        field.setAttribute('aria-label',`${this.metadata!.name} ${p.name}`);
        if(field instanceof HTMLSelectElement){p.choices!.forEach((name,i)=>field.add(new Option(name,String(p.minimum+i))));field.value=String(Math.round(value));}
        else{field.type='number';field.min=String(p.minimum*scale);field.max=String(p.maximum*scale);field.step=p.integer?'1':String(Math.max(.001,(p.maximum-p.minimum)*scale/1000));field.value=String(Number((value*scale).toPrecision(6)));}
        field.disabled=!this.enabled();
        let numericGesture:TouchGesture|undefined;
        field.onchange=()=>{
            if(generation!==this.generation)return;const physical=Number(field.value)/(p.choices?1:scale);
            if(!Number.isFinite(physical)||physical<p.minimum||physical>p.maximum){this.status.textContent=`${p.name} is outside its range.`;return;}
            if(this.touch?.armed(trackID,target)){numericGesture=this.touch.start(trackID,target);numericGesture?.offer(parameterNormalized(physical,p));void numericGesture?.end();return;}
            this.changeParameter(p,p.choices||p.integer?Math.round(physical):physical);
        };
        label.append(field);row.append(label);const display=el('output',parameterDisplay(value,p));display.className='parameter-readout';let slider:HTMLInputElement|undefined;
        if(!p.choices){
            slider=el('input');slider.type='range';slider.min='0';slider.max='1';slider.step='.001';slider.value=String(parameterNormalized(value,p));slider.setAttribute('aria-label',`${this.metadata!.name} ${p.name} slider`);slider.disabled=!this.enabled();
            const binding=this.touch?new TouchControlBinding(this.touch,trackID,target):undefined;
            slider.onpointerdown=()=>{if(generation!==this.generation)return;binding?.pointerDown();if(!this.touch?.armed(trackID,target))this.beginGesture(`Adjust ${p.name}`);};
            slider.onkeydown=e=>{if(generation!==this.generation)return;binding?.keyDown(e.key,e.repeat);if(!e.repeat&&['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End','PageUp','PageDown'].includes(e.key)&&!this.touch?.armed(trackID,target))this.beginGesture(`Adjust ${p.name}`);};
            slider.onkeyup=()=>binding?.keyUp(Number(slider!.value));
            slider.oninput=()=>{if(generation!==this.generation)return;const normalized=Number(slider!.value),physical=parameterValue(normalized,p);if(!binding?.input(normalized)){if(!this.gesture)this.beginGesture(`Adjust ${p.name}`);this.changeParameter(p,physical,true);}field.value=String(Number((physical*scale).toPrecision(6)));display.textContent=parameterDisplay(physical,p);};
            slider.onchange=()=>{if(generation!==this.generation)return;if(!binding?.change(Number(slider!.value)))this.offer();};
            slider.onpointerup=()=>binding?.pointerUp(Number(slider!.value));slider.onpointercancel=()=>binding?.cancel();slider.onblur=()=>binding?.blur();row.append(slider);
        }
        const automation=this.button('A',()=>void this.openAutomation(p.id));automation.title=`Edit ${p.name} automation`;automation.setAttribute('aria-label',`Edit ${p.name} automation`);row.append(display,automation);
        this.updates.set(p.id,value=>{field.value=String(Number((value*(p.choices?1:scale)).toPrecision(6)));if(slider)slider.value=String(parameterNormalized(value,p));display.textContent=parameterDisplay(value,p);});return row;
    }
    private beginGesture(name: string) { this.gesture = randomUUID().toUpperCase(); this.gestureName = name; }
    private offer() { if (this.draft)
        this.writer?.offer({ effect: structuredClone(this.draft), name: this.gestureName, gesture: this.gesture }); }
    private changeParameter(p: Parameter, value: number, continuous = false) { const parameters = { ...this.draft!.parameters, [p.id]: value }; this.change({ parameters }, `Adjust ${p.name}`, continuous); }
    private change(values: Partial<RealtimeEffect>, name: string, continuous = false) { if (!this.enabled() || !this.draft)
        return; this.draft = { ...this.draft, ...values }; this.dirty = true; if (!continuous || !this.writer)
        this.beginGesture(name); this.offer(); }
    private async commit(effect:RealtimeEffect,name:string,gestureID:string) {
        const track=this.track(),before=this.trackRevision,generation=this.generation;
        if(!track||!this.enabled()) return false;
        if(!this.ready()) {this.conflict();return false;}
        this.writing++;
        try {
            const reply=await this.send({type:'updateTrackControls',controls:{...trackControls(track),realtimeEffects:track.realtimeEffects.map(e=>(e as RealtimeEffect).id===effect.id?effect:e)},name,gestureID},track.id,before);
            if(generation!==this.generation) return false;
            const next=ownAcceptedTrackRevision(before,this.mirror.snapshot?.trackRevisions[this.trackID],reply,sameEffectSettings(this.current(),effect));
            if(!reply||this.sourceEpoch!==this.epoch?.()||next===undefined||!this.current()) {this.conflict();return false;}
            this.trackRevision=next;this.status.textContent='Live on the hardware server. Automation stays on the track.';return true;
        } catch(error) {if(generation===this.generation)this.conflict(error instanceof Error?error.message:undefined);return false;}
        finally {if(generation===this.generation)this.writing--;}
    }
    private async applyChain(chain:SavedEffectChain) {
        const generation=this.generation;
        if(this.writer&&!await this.writer.flush()||generation!==this.generation) return false;
        const track=this.track(),before=this.trackRevision;if(!track||!this.enabled()||!this.ready()) {this.conflict();return false;}
        const effects=prepareEffectChain(chain,this.effects());this.writing++;
        try {
            const reply=await this.send({type:'updateTrackControls',controls:{...trackControls(track),realtimeEffects:effects},name:`Chain ${chain.name}`},track.id,before);
            if(generation!==this.generation)return false;
            const current=this.track(),unchanged=!!current&&current.realtimeEffects.length===effects.length&&current.realtimeEffects.every((e,i)=>sameEffectSettings(e as RealtimeEffect,effects[i]!));
            const next=ownAcceptedTrackRevision(before,this.mirror.snapshot?.trackRevisions[track.id],reply,unchanged);
            if(!reply||this.sourceEpoch!==this.epoch?.()||next===undefined||!current) {this.conflict();return false;}
            this.trackRevision=next;return true;
        }catch(error){if(generation===this.generation)this.conflict();return false;}
        finally{if(generation===this.generation)this.writing--;}
    }
    private customPresets(kind: string): {
        name: string;
        values: Record<string, number>;
    }[] { try {
        const value = JSON.parse(localStorage.getItem(`soundroom.presets.${kind}`) ?? '[]');
        return Array.isArray(value) ? value.filter(p => p && typeof p.name === 'string' && p.name.length <= 100 && p.values && Object.values(p.values).every(v => typeof v === 'number' && Number.isFinite(v))).slice(-128) : [];
    }
    catch {
        return [];
    } }
    private async openManager() { const generation=this.generation; if (this.writer && !await this.writer.flush() || generation!==this.generation || !this.ready()) return; this.managerGeneration=this.generation; this.manager?.open(); }
    private async applySaved(preset: SavedEffectPreset) {
        const generation=this.generation; if (this.writer && !await this.writer.flush() || generation!==this.generation || !this.ready()) return false;
        const current = this.current(); if (!current || !this.metadata) return false;
        const effect = applyEffectPreset(current, preset, this.effects());
        this.draft=editableEffect(effect,this.metadata); const draft=this.draft;
        const accepted = await this.commit(effect, `Preset ${preset.name}`, randomUUID().toUpperCase());
        if (accepted && generation===this.generation && this.draft===draft) { this.render(); }
        return accepted;
    }
    private async savePreset() {
        const generation=this.generation; if (this.writer && !await this.writer.flush() || generation!==this.generation || !this.ready()) return;
        const name = prompt('Preset name'); if (!name?.trim() || !this.metadata || !this.draft) return;
        try {
            if (this.library) { const source = this.current(); if (!source) return; this.library.savePreset(presetFromEffect(name, source, this.effects())); }
            else { const preset = { name: name.trim().slice(0, 100), values: Object.fromEntries(this.metadata.parameters.map(p => [p.id, this.draft!.parameters?.[p.id] ?? p.defaultValue])) }; const existing = this.customPresets(this.metadata.id).filter(p => p.name !== preset.name); localStorage.setItem(`soundroom.presets.${this.metadata.id}`, JSON.stringify([...existing, preset].slice(-128))); }
            this.render(); this.status.textContent = 'Custom preset saved in this browser.';
        } catch (error) { this.status.textContent = String(error); }
    }
    private async transport(trackOnly = false) { if (!this.canOperate())
        return; if (this.mirror.snapshot?.transport.isPlaying) {
        await this.send({ type: 'editor', intent: { operation: 'transport.pause', context: { cursorFrame: 0, clipIDs: [] }, integers: {}, numbers: {}, strings: {}, flags: {} } });
        return;
    } await this.send({ type: 'editor', intent: { operation: 'transport.play', context: { cursorFrame: this.mirror.view.cursor, clipIDs: [] }, integers: {}, numbers: {}, strings: trackOnly ? { trackID: this.trackID } : {}, flags: {} } }); }
    private renderMeters() { const meter = this.meter, stale = !meter || performance.now() - this.meteredAt > 1000; this.metersPanel.replaceChildren(); for (const [label, value] of [['Input', meter ? `${(20 * Math.log10(Math.max(1e-6, meter.inputPeak))).toFixed(1)} dBFS` : '—'], ['Output', meter ? `${(20 * Math.log10(Math.max(1e-6, meter.outputPeak))).toFixed(1)} dBFS` : '—'], ['Reduction', meter ? `${meter.gainReductionDB.toFixed(1)} dB` : '—'], ['True peak', meter ? `${(20 * Math.log10(Math.max(1e-6, meter.truePeak))).toFixed(1)} dBTP` : '—'], ['Momentary', meter ? `${meter.momentaryLUFS.toFixed(1)} LUFS` : '—']] as [
        string,
        string
    ][]) {
        const item = el('div');
        item.append(el('span', label), el('strong', value));
        this.metersPanel.append(item);
    } const status = el('span', stale ? 'Last measurement · play to refresh' : 'Measured at server'); status.className = 'meter-origin'; this.metersPanel.append(status); }
    private plot(points: {
        x: number;
        y: number;
    }[], xmin: number, xmax: number, ymin: number, ymax: number, logarithmic = false, color = '#9ac7c2') { const c = this.context; const x = (v: number) => 42 + (logarithmic ? Math.log(v / xmin) / Math.log(xmax / xmin) : (v - xmin) / (xmax - xmin)) * (this.width - 60), y = (v: number) => 20 + (ymax - v) / (ymax - ymin) * (this.height - 45); c.save(); c.beginPath(); c.rect(42, 20, this.width - 60, this.height - 45); c.clip(); c.strokeStyle = color; c.lineWidth = 2; c.beginPath(); points.forEach((point, index) => { if (index === 0)
        c.moveTo(x(point.x), y(point.y));
    else
        c.lineTo(x(point.x), y(point.y)); }); c.stroke(); c.restore(); return { x, y }; }
    private axes(xmin: number, xmax: number, ymin: number, ymax: number, unit: string, logarithmic = false) { const c = this.context; c.fillStyle = '#121b25'; c.fillRect(0, 0, this.width, this.height); c.font = '11px system-ui'; for (let i = 0; i <= 4; i++) {
        const value = ymin + (ymax - ymin) * i / 4, y = 20 + (4 - i) / 4 * (this.height - 45);
        c.strokeStyle = '#3a4b5d';
        c.beginPath();
        c.moveTo(42, y);
        c.lineTo(this.width - 16, y);
        c.stroke();
        c.fillStyle = '#9fb1c5';
        c.fillText(String(Number(value.toFixed(1))), 4, y + 4);
    } const ticks = logarithmic ? [20, 100, 1000, 10000, 20000] : [xmin, (xmin + xmax) / 2, xmax]; for (const value of ticks) {
        const x = 42 + (logarithmic ? Math.log(value / xmin) / Math.log(xmax / xmin) : (value - xmin) / (xmax - xmin)) * (this.width - 60);
        c.fillStyle = '#9fb1c5';
        c.fillText(value >= 1000 && unit === 'Hz' ? `${value / 1000}k` : String(Number(value.toFixed(2))), Math.min(this.width - 36, Math.max(42, x)), this.height - 6);
    } c.fillText(unit, this.width - 42, 13); }
    private draw() {
        if (!this.draft)
            return;
        const effect = this.draft, p = effect.parameters ?? {}, kind = effect.kind, c = this.context, rate = this.mirror.sampleRate;
        let caption = 'Settings model · this graph does not measure the audio signal.';
        if (['equalizer', 'lowPass', 'highPass', 'gain'].includes(kind)) {
            this.axes(20, Math.min(20000, rate / 2), -24, 24, 'Hz', true);
            const points = Array.from({ length: 500 }, (_, i) => { const frequency = 20 * (Math.min(20000, rate / 2) / 20) ** (i / 499), gain = kind === 'equalizer' ? eqResponse(frequency, p, rate) : kind === 'gain' ? p.gain ?? 0 : 20 * Math.log10(Math.max(1e-12, sectionResponse(p.frequency ?? 1000, p.resonance ?? 0.707, 0, kind === 'lowPass' ? 'lowPass' : 'highPass', frequency, rate))); return { x: frequency, y: gain }; }), axes = this.plot(points, 20, Math.min(20000, rate / 2), -24, 24, true);
            if (kind === 'equalizer') {
                for (const band of ['low', 'mid', 'high']) {
                    c.fillStyle = '#b8dec9';
                    c.beginPath();
                    c.arc(axes.x(p[`${band}Frequency`]!), axes.y(p[`${band}Gain`]!), 5, 0, Math.PI * 2);
                    c.fill();
                }
                caption += ' Drag band points to adjust frequency and gain.';
            }
        }
        else if (kind === 'compressor' || kind === 'limiter') {
            this.axes(-60, 0, -60, 6, 'dBFS');
            this.plot(Array.from({ length: 300 }, (_, i) => { const input = -60 + i / 299 * 60; return { x: input, y: kind === 'compressor' ? compressorOutput(input, p) : Math.min(p.ceiling ?? -1, input + (p.input ?? 0)) }; }), -60, 0, -60, 6, false, '#dfba78');
            caption = 'Configured steady-state transfer · attack, release and true-peak behavior are measured separately.';
        }
        else if (kind === 'warmth') {
            this.axes(-1, 1, -1.4, 1.4, 'amplitude');
            this.plot(Array.from({ length: 300 }, (_, i) => { const x = -1 + i / 299 * 2; return { x, y: warmthCurve(x, p) }; }), -1, 1, -1.4, 1.4, false, '#e6a081');
            caption = 'Configured nonlinear curve · before tone, DC filtering and level matching.';
        }
        else if (kind === 'echo') {
            const end = Math.max(1, (p.time ?? 250) * 7);
            this.axes(0, end, 0, 1, 'ms');
            for (let i = 1; i <= 7; i++) {
                const x = 42 + (p.time ?? 250) * i / end * (this.width - 60), level = (p.mix ?? 0.25) * (p.feedback ?? 0.4) ** (i - 1);
                c.fillStyle = '#bca4e2';
                c.fillRect(x - 2, 20 + (1 - level) * (this.height - 45), 4, level * (this.height - 45));
            }
            caption = 'First seven configured wet repeats · before feedback-loop tone filtering.';
        }
        else if (kind === 'room') {
            const start = (p.predelay ?? 0) / 1000, decay = p.decay ?? 1.4;
            this.axes(0, start + decay, -60, 0, 'seconds');
            this.plot([{ x: 0, y: -60 }, { x: start, y: -60 }, { x: start, y: 0 }, { x: start + decay, y: -60 }], 0, start + decay, -60, 0, false, '#bca4e2');
            caption = 'Nominal late-tail decay guide to −60 dB · reflections and diffusion are not represented.';
        }
        else if (kind === 'chorus') {
            const depth = p.depth ?? 3, delay = p.delay ?? 16;
            this.axes(0, 1, delay, delay + Math.max(1, depth), 'LFO cycle');
            for (const [right, color] of [[false, '#bca4e2'], [true, '#7cbfca']] as const)
                this.plot(Array.from({ length: 200 }, (_, i) => { const phase = i / 199; return { x: phase, y: delay + depth * (Math.sin(phase * Math.PI * 2 + (right ? Math.PI * (p.width ?? 0.7) : 0)) + 1) / 2 }; }), 0, 1, delay, delay + Math.max(1, depth), false, color);
            caption = `Configured stereo delay modulation · rate ${(p.rate ?? 0.8).toFixed(2)} Hz.`;
        }
        else if (kind === 'resonance') {
            this.axes(80, 20000, 0, Math.max(1, p.depth ?? 6), 'Hz', true);
            const bands = Array.from({ length: 12 }, (_, i) => this.meter?.resonance[`b${i}` as keyof EffectMeterSnapshot['resonance']] ?? 0), max = Math.max(1, p.depth ?? 6);
            for (let i = 0; i < 12; i++) {
                const x = 42 + i / 12 * (this.width - 60), height = bands[i]! / max * (this.height - 45);
                c.fillStyle = '#8db8de';
                c.fillRect(x + 2, this.height - 25 - height, (this.width - 60) / 12 - 4, height);
            }
            caption = this.meter ? 'Measured server suppression · greatest attenuation in each of 12 logarithmic bands.' : 'Measured spectral suppression · play the server mix to populate the bands.';
        }
        else if (kind === 'dither') {
            const bits = Math.round(p.bits ?? 24), step = 2 ** (1 - bits);
            this.axes(-3, 3, -3, 3, 'LSB');
            this.plot(Array.from({ length: 150 }, (_, i) => { const x = -3 + i / 149 * 6; return { x, y: Math.round(x) }; }), -3, 3, -3, 3, false, '#a6b3bf');
            caption = `Quantization grid guide · one step ${step.toExponential(2)} full scale. ${p.quantize ? 'Dither + quantize' : 'Dither only; quantization is not applied'}.`;
        }
        else {
            this.axes(0, 1, -12, 12, '');
            const pitch = this.meter?.pitch;
            c.fillStyle = '#d6a2c2';
            c.font = '18px system-ui';
            c.fillText(pitch ? `Measured correction ${pitch.correctionCents.toFixed(1)} cents` : 'Open Pitch chart for measured contour and manual targets.', 55, this.height / 2);
            caption = 'Pitch detector readings come from the hardware server; the Pitch chart provides offline analysis.';
        }
        this.caption.textContent = caption;
        this.graph.setAttribute('aria-label', caption);
    }
    private down(e: PointerEvent) { if (!this.enabled() || e.button !== 0 || this.draft?.kind !== 'equalizer')
        return; const p = this.draft.parameters!, x = (frequency: number) => 42 + Math.log(frequency / 20) / Math.log(Math.min(20000, this.mirror.sampleRate / 2) / 20) * (this.width - 60), y = (gain: number) => 20 + (24 - gain) / 48 * (this.height - 45); let distance = 225, chosen: string | undefined; for (const band of ['low', 'mid', 'high']) {
        const d = (x(p[`${band}Frequency`]!) - e.offsetX) ** 2 + (y(p[`${band}Gain`]!) - e.offsetY) ** 2;
        if (d < distance) {
            distance = d;
            chosen = band;
        }
    } if (chosen) {
        this.graphDrag = chosen;
        this.beginGesture(`Adjust ${chosen} EQ band`);
        this.graph.setPointerCapture(e.pointerId);
    } }
    private move(e: PointerEvent) { if (!this.graphDrag || !this.draft || !this.metadata)
        return; const band = this.graphDrag, frequency = 20 * (Math.min(20000, this.mirror.sampleRate / 2) / 20) ** ((e.offsetX - 42) / (this.width - 60)), gain = 24 - (e.offsetY - 20) / (this.height - 45) * 48, values: Record<string, number> = {}; for (const [key, value] of [[`${band}Frequency`, frequency], [`${band}Gain`, gain]] as const) {
        const parameter = this.metadata.parameters.find(p => p.id === key)!;
        values[key] = Math.min(parameter.maximum, Math.max(parameter.minimum, value));
        this.updates.get(key)?.(values[key]!);
    } this.change({ parameters: { ...this.draft.parameters, ...values } }, `Adjust ${band} EQ band`, true); }
    private tick() { if (this.dialog.open) {
        const stale = !this.meter || performance.now() - this.meteredAt > 1000;
        if (stale !== this.staleMeters) {
            this.staleMeters = stale;
            this.renderMeters();
        }
    } if (this.dialog.open && this.dirty) {
        this.draw();
        this.dirty = false;
    } requestAnimationFrame(() => this.tick()); }
    refresh() { if (!this.dialog.open)
        return; if (!this.current()) {
        this.status.textContent = 'This effect was removed. Close the workspace.';
        for (const c of this.dialog.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>('input,select,button'))
            if (c.textContent !== 'Done')
                c.disabled = true;
        return;
    } if (!this.writing && !this.ready()) this.conflict(); if (!this.enabled())
        for (const c of this.controls.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>('input,select,button'))
            c.disabled = true; }
    private async openPitchChart() {const generation=this.generation,effectID=this.effectID,trackID=this.trackID;if(this.writer&&!await this.writer.flush()||generation!==this.generation||!this.ready())return;await this.close();if(this.generation===generation+1)this.openPitch(effectID,trackID);}
    private async openAutomation(parameter:string) {
        const generation=this.generation,trackID=this.trackID,effectID=this.effectID;
        if(this.writer&&!await this.writer.flush()||generation!==this.generation||!this.ready()) return;
        const current=this.current();if(current&&!Object.keys(current.parameters??{}).length&& !await this.commit(this.draft!,'Enable precision automation',randomUUID().toUpperCase())) return;
        if(generation!==this.generation)return;await this.close();if(this.generation===generation+1)this.automate(trackID,effectID,parameter);
    }
    private async bake() {
        const generation=this.generation;
        if(this.writer&&!await this.writer.flush()||generation!==this.generation)return;
        if(!this.enabled()||!this.track()?.clips.length||this.writing)return;
        if(!this.ready()){this.conflict();return;}
        if(!confirm('Apply the complete live insert chain and automation to audio? Undo and clip history retain the source.')) return;
        const before=this.trackRevision;this.writing++;
        try {
            const reply=await this.send({type:'editor',intent:{operation:'pitch.apply',context:{trackID:this.trackID,effectID:this.effectID,cursorFrame:0,clipIDs:[]},integers:{},numbers:{},strings:{trackID:this.trackID,effectID:this.effectID},flags:{}}},this.trackID,before);
            if(generation!==this.generation)return;
            if(!reply||this.sourceEpoch!==this.epoch?.()||ownAcceptedTrackRevision(before,this.mirror.snapshot?.trackRevisions[this.trackID],reply)===undefined){this.conflict();return;}
            await this.close();
        }catch(error){if(generation===this.generation)this.conflict();}
        finally{if(generation===this.generation)this.writing--;}
    }
    async close() {this.generation++;this.writer?.cancel();this.graphDrag=undefined;this.dialog.close();await this.subscribe([]);}
}
