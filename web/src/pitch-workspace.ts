import {randomUUID} from './crypto.js';
import { acceptWorkspaceShortcut, isItemNavigation, navigateTimelineItem, openWorkspaceDialog } from './input-focus.js';
import { inputScale, sameEffectSettings } from './effect-models.js';
import { type SessionMirror, type Command, type Track } from './model.js';
import type { RealtimeEffect, PitchEditRegion, PitchEditPoint, RemotePitchAnalysis, RemotePitchPoint, AutomationCurve } from './generated/models.js';
import { type EffectMetadata, trackControls } from './effects.js';
import { payload } from './transfers.js';
import { validateModel } from './validate.js';
import { GestureWriter, preparedAutomation, type Point } from './automation.js';
import { formatPosition, parsePosition, zoomAround } from './timing.js';
import { frequencyNote, replacePitchScope, sliceRegion, editPitchPoint, pitchPointCount, addPitchPoint } from './pitch.js';
import { noteName } from './music.js';
import { ownAcceptedTrackRevision, TrackGestureRevision } from './gesture-revision.js';
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, text?: string) => { const e = document.createElement(tag); if (text)
    e.textContent = text; return e; };
type Sender = (command: Command, trackID?: string, expectedTrackRevision?:number) => Promise<{
    payload?: unknown; revision?:number;
} | undefined>;
export class PitchWorkspace {
    private dialog: HTMLDialogElement;
    private toolbar: HTMLElement;
    private controls: HTMLElement;
    private chart: HTMLElement;
    private canvas: HTMLCanvasElement;
    private context: CanvasRenderingContext2D;
    private status: HTMLElement;
    private revision = 0;
    private lastTrackRevision = 0;
    private sourceEpoch?:number;
    private blocked = false;
    private writing = 0;
    private keyboardWriter?: GestureWriter<PitchEditRegion[]>;
    private trackID = '';
    private effectID?: string;
    private scope: [
        number,
        number
    ] = [0, 0];
    private regions: PitchEditRegion[] = [];
    private before: RemotePitchPoint[] = [];
    private after: RemotePitchPoint[] = [];
    private parameters: Record<string, number> = {};
    private selected?: string;
    private busy = false;
    private auditioning = false;
    private draft = false;
    private scale = 1;
    private offset = 0;
    private top = 84;
    private semitonePixels = 20;
    private width = 700;
    private height = 420;
    private generation = 0;
    private dirty = true;
    private drag?: {
        id: string;
        writer: GestureWriter<PitchEditRegion[]>;
    };
    private resize: ResizeObserver;
    private pointPosition: HTMLInputElement;
    private pointNote: HTMLInputElement;
    private pointLabel: HTMLElement;
    private curve: HTMLSelectElement;
    private snap: HTMLInputElement;
    private raf = 0;
    constructor(private mirror: SessionMirror, private send: Sender, private report: (message: string) => void, private metadata: () => EffectMetadata[], private enabled: () => boolean, private canOperate: () => boolean, private operations: () => string[], private epoch?:()=>number) {
        this.dialog = el('dialog');
        this.dialog.className = 'pitch-dialog';
        this.dialog.setAttribute('aria-label','Pitch analysis and manual correction');
        this.toolbar = el('div');
        this.toolbar.className = 'workspace-toolbar';
        this.controls = el('div');
        this.controls.className = 'pitch-controls';
        this.chart = el('div');
        this.chart.className = 'pitch-chart';
        this.canvas = el('canvas');
        this.canvas.tabIndex = 0;
        this.canvas.setAttribute('role', 'application');
        this.canvas.setAttribute('aria-label', 'Pitch target editor. Home or End selects the first or last target; brackets select previous or next. Arrows adjust the selected target; Delete removes it. Drag points, double-click voiced regions to add. Ctrl-wheel zooms time; Alt-wheel zooms pitch.');
        this.chart.append(this.canvas);
        const ctx = this.canvas.getContext('2d');
        if (!ctx)
            throw Error('Pitch chart unavailable.');
        this.context = ctx;
        this.status = el('div');
        this.status.className = 'workspace-status';
        this.status.setAttribute('role', 'status');
        this.pointPosition = el('input');
        this.pointPosition.setAttribute('aria-label', 'Pitch point timeline position');
        this.pointPosition.onchange = () => this.editNumeric();
        this.pointNote = el('input');
        this.pointNote.type = 'number';
        this.pointNote.min = '0';
        this.pointNote.max = '95';
        this.pointNote.step = '0.01';
        this.pointNote.setAttribute('aria-label', 'Pitch target MIDI note, fractional semitones');
        this.pointNote.onchange = () => this.editNumeric();
        this.pointLabel = el('output');
        this.curve = el('select');
        this.curve.setAttribute('aria-label', 'Pitch target curve');
        for (const c of ['linear', 'smooth', 'easeIn', 'easeOut', 'hold'])
            this.curve.add(new Option(c, c));
        this.curve.onchange = () => this.editNumeric();
        this.snap = el('input');
        this.snap.type = 'checkbox';
        this.snap.setAttribute('aria-label', 'Snap manual target edits to semitones');
        this.dialog.append(this.toolbar, this.controls, this.chart, this.status);
        document.body.append(this.dialog);
        this.resize = new ResizeObserver(() => { this.width = this.chart.clientWidth; this.height = this.chart.clientHeight; const dpr = Math.min(devicePixelRatio, 2); this.canvas.width = this.width * dpr; this.canvas.height = this.height * dpr; this.canvas.style.width = `${this.width}px`; this.canvas.style.height = `${this.height}px`; this.context.setTransform(dpr, 0, 0, dpr, 0, 0); this.dirty = true; });
        this.resize.observe(this.chart);
        this.canvas.onpointerdown = e => this.down(e);
        this.canvas.onpointermove = e => this.move(e);
        this.canvas.onpointerup = e => { this.drag = undefined; if (this.canvas.hasPointerCapture(e.pointerId))
            this.canvas.releasePointerCapture(e.pointerId); };
        this.canvas.onpointercancel = () => { this.drag?.writer.cancel(); this.keyboardWriter?.cancel(); this.drag = undefined; };
        this.canvas.ondblclick = e => this.addPoint(e);
        this.canvas.onwheel = e => { e.preventDefault(); if (e.altKey) {
            const anchor = this.noteAt(e.offsetY), next = Math.min(100, Math.max(5, this.semitonePixels * Math.exp(-e.deltaY * 0.003)));
            this.top = anchor + (e.offsetY - 26) / next;
            this.semitonePixels = next;
        }
        else if (e.ctrlKey || e.metaKey)
            this.zoom(Math.exp(-e.deltaY * 0.003), e.offsetX);
        else if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY))
            this.offset = Math.max(0, this.offset + (e.shiftKey ? e.deltaY : e.deltaX));
        else
            this.top = Math.max(5, Math.min(100, this.top + e.deltaY / this.semitonePixels)); this.dirty = true; };
        this.dialog.oncancel = e => { e.preventDefault(); void this.close(); };
        this.dialog.addEventListener('keydown', e => this.key(e));
        this.raf = requestAnimationFrame(() => this.tick());
    }
    private track() { return this.mirror.snapshot?.session.tracks.find(t => t.id === this.trackID); }
    private effect() { return this.track()?.realtimeEffects.find(e => (e as RealtimeEffect).id === this.effectID) as RealtimeEffect | undefined; }
    private meta() { return this.metadata().find(m => m.id === 'pitchCorrection'); }
    open(requestedEffect?: string, requestedTrackID?:string) { const track = this.mirror.snapshot?.session.tracks.find(t => t.id === (requestedTrackID??this.mirror.view.selectedTrack)), clip = track?.clips.find(c => this.mirror.view.selectedClips.has(c.id)); if (!track?.clips.length) {
        this.report('Select a track with audio to edit pitch.');
        return;
    } if (!this.meta()) {
        this.report('This server does not advertise pitch correction.');
        return;
    } this.generation++; this.drag?.writer.cancel(); this.keyboardWriter?.cancel(); this.drag=undefined; this.keyboardWriter=undefined; this.writing=0; this.blocked=false; this.sourceEpoch=this.epoch?.(); this.trackID = track.id; this.effectID = requestedEffect ?? (track.realtimeEffects.find(e => (e as RealtimeEffect).kind === 'pitchCorrection') as RealtimeEffect | undefined)?.id; this.scope = this.mirror.view.range && this.mirror.view.range[1] > this.mirror.view.range[0] ? [...this.mirror.view.range] : clip ? [clip.start, clip.start + clip.length] : [0, Math.max(...track.clips.map(c => c.start + c.length))]; this.regions = (this.effect()?.pitchEdits ?? []).map(r => sliceRegion(r, ...this.scope)).filter((r): r is PitchEditRegion => !!r); this.parameters = { ...Object.fromEntries(this.meta()!.parameters.map(p => [p.id, p.defaultValue])), ...this.effect()?.parameters }; this.before = []; this.after = []; this.selected = undefined; this.draft = false; this.busy = false; this.auditioning = false; this.offset = 0; this.scale = Math.max(0.00001, (this.width - 60) / Math.max(1, this.scope[1] - this.scope[0])); this.revision = this.mirror.snapshot!.revision; this.lastTrackRevision = this.mirror.snapshot!.trackRevisions[track.id] ?? 0; this.renderControls(); this.renderToolbar(); openWorkspaceDialog(this.dialog,this.canvas); requestAnimationFrame(()=>{if(this.dialog.open)this.fit();}); this.dirty = true; this.message('Analyze the selected audio to reveal measured pitch and editable targets.'); }
    private ready() { return !this.blocked && this.sourceEpoch===this.epoch?.() && !!this.track() && this.mirror.snapshot?.trackRevisions[this.trackID]===this.lastTrackRevision; }
    private conflict() { this.blocked=true; this.draft=true; this.message('The source changed or this edit could not be confirmed. Your targets are kept here. Analyze again to review the current source.'); }
    private message(text: string) { this.status.textContent = text; this.report(text); }
    private button(label: string, action: () => void, operate = false) { const b = el('button', label); b.disabled = this.busy || !this.enabled() || operate && !this.canOperate(); if (operate && !this.canOperate())
        b.title = 'Audition requires an operator role.'; b.onclick = action; return b; }
    private renderToolbar() { this.toolbar.replaceChildren(); this.toolbar.append(el('strong', 'Pitch · ' + (this.track()?.name ?? '')), this.button('Analyze', () => void this.analyze('pitch.analyze')), this.button('Measure output', () => void this.analyze('pitch.render')), this.button('Audition', () => void this.analyze('pitch.render', true)), this.button('Original', () => void this.analyze('pitch.render', true, true))); const targetPreset = el('select'); targetPreset.setAttribute('aria-label', 'Pitch target preset'); targetPreset.add(new Option('Targets…', '')); for (const p of ['Natural', 'Tight', 'Robotic', 'Keep performance'])
        targetPreset.add(new Option(p, p)); targetPreset.disabled = this.busy || !this.enabled() || !this.operations().includes('pitch.targets'); targetPreset.onchange = () => void this.analyze('pitch.targets', false, false, targetPreset.value); this.toolbar.append(targetPreset, this.button('Use targets', () => void this.commit(this.regions, 'Activate pitch targets'))); for (const [label, action] of [['Time −', () => this.zoom(1 / 1.5)], ['Time +', () => this.zoom(1.5)], ['Pitch −', () => { this.semitonePixels = Math.max(5, this.semitonePixels / 1.3); this.dirty = true; }], ['Pitch +', () => { this.semitonePixels = Math.min(100, this.semitonePixels * 1.3); this.dirty = true; }], ['Fit', () => this.fit()]] as [
        string,
        () => void
    ][]) {
        const button = el('button', label);
        button.onclick = action;
        this.toolbar.append(button);
    } const apply = this.button('Apply to audio', () => void this.bake()); apply.disabled = this.busy || !this.enabled() || !this.effectID || this.draft || this.writing>0 || !this.ready(); this.toolbar.append(apply); const cancel = el('button', 'Cancel job'); cancel.disabled = !this.busy; cancel.onclick = () => void this.send({ type: 'editor', intent: { operation: 'jobs.cancel', context: { cursorFrame: 0, clipIDs: [] }, integers: {}, numbers: {}, strings: {}, flags: {} } }); this.toolbar.append(cancel); const close = el('button', 'Done'); close.onclick = () => void this.close(); this.toolbar.append(close); }
    private renderControls() {
        this.controls.replaceChildren();
        const selected = el('section');
        selected.append(el('h3', 'Target point'));
        for (const [label, input] of [['Position', this.pointPosition], ['MIDI note', this.pointNote], ['Curve', this.curve]] as const) {
            const l = el('label', label);
            l.append(input);
            selected.append(l);
        }
        selected.append(this.pointLabel, this.button('Delete point', () => void this.removePoint()));
        const snap = el('label', 'Snap to notes');
        snap.append(this.snap);
        selected.append(snap);
        selected.append(this.button('Smooth targets', () => void this.commit(this.regions.map(r => ({ ...r, points: r.points?.map(p => ({ ...p, curve: 'smooth', curveStart: 0, curveEnd: 1 })) })), 'Smooth pitch targets')));
        this.controls.append(selected);
        const presets = el('select');
        presets.setAttribute('aria-label', 'Pitch processing preset');
        presets.add(new Option('Processing preset…', ''));
        for (const [i, p] of (this.meta()?.factoryPresets ?? []).entries())
            presets.add(new Option(p.name, String(i)));
        presets.disabled = this.busy || !this.enabled();
        presets.onchange = () => { const preset = this.meta()?.factoryPresets?.[Number(presets.value)]; if (preset) {
            this.parameters = { ...this.parameters, ...preset.values };
            void this.commit(this.regions, 'Pitch processing preset').then(() => this.renderControls());
        } };
        this.controls.append(presets);
        for (const p of this.meta()?.parameters ?? []) {
            const l = el('label', p.name), input = p.choices ? el('select') : el('input');
            if (input instanceof HTMLSelectElement) {
                p.choices!.forEach((choice, i) => input.add(new Option(choice, String(p.minimum+i))));
            }
            else {
                input.type = 'number';
                input.min = String(p.minimum*inputScale(p));
                input.max = String(p.maximum*inputScale(p));
                input.step = p.integer ? '1' : String(Math.max(0.01, (p.maximum - p.minimum)*inputScale(p) / 1000));
            }
            input.value = String((this.parameters[p.id] ?? p.defaultValue)*inputScale(p));
            input.disabled = this.busy || !this.enabled();
            input.onchange = () => { const value = Number(input.value)/(p.choices?1:inputScale(p)); if (!Number.isFinite(value) || value < p.minimum || value > p.maximum) {
                this.message(`${p.name} is outside its range.`);
                return;
            } this.parameters[p.id] = p.integer||p.choices ? Math.round(value) : value; void this.commit(this.regions, `Pitch ${p.name}`); };
            l.append(input);
            if (p.unit)
                l.append(el('span', p.unit));
            this.controls.append(l);
        }
        const automatic = el('label', 'Automatic outside targets'), check = el('input');
        check.type = 'checkbox';
        check.checked = this.effect()?.pitchEditsOnly === false;
        check.disabled = this.busy || !this.enabled();
        check.onchange = () => void this.commit(this.regions, 'Pitch automatic mode', undefined, !check.checked);
        automatic.append(check);
        this.controls.append(automatic);
        this.updatePoint();
    }
    private async analyze(operation: string, preview = false, original = false, preset?: string) {
        if (this.busy || this.writing || !this.enabled() || preview && !this.canOperate()) return;
        const reload=operation==='pitch.analyze';
        if (!reload && !this.ready()) { this.conflict(); return; }
        if (reload && (this.blocked || this.draft) && !confirm('Analyze the current source and replace the targets kept in this editor?')) return;
        const generation=++this.generation, epoch=this.epoch?.(), before=this.mirror.snapshot?.trackRevisions[this.trackID], requestEffectID=reload?(this.track()?.realtimeEffects.find(e=>(e as RealtimeEffect).kind==='pitchCorrection') as RealtimeEffect|undefined)?.id:this.effectID;
        if (before===undefined || !this.track()) { this.conflict(); return; }
        this.drag?.writer.cancel(); this.keyboardWriter?.cancel(); this.drag=undefined; this.keyboardWriter=undefined;
        this.busy=true; this.renderToolbar(); this.renderControls();
        this.message(reload?'Analyzing selected audio on the server…':operation==='pitch.targets'?'Preparing stabilized pitch targets…':'Preparing pitch output…');
        try {
            const reply=await this.send({type:'editor',intent:{operation,context:{trackID:this.trackID,effectID:requestEffectID,cursorFrame:this.scope[0],clipIDs:[],rangeStart:this.scope[0],rangeEnd:this.scope[1]},integers:{start:this.scope[0],end:this.scope[1]},numbers:{},strings:{trackID:this.trackID,...(requestEffectID?{effectID:requestEffectID}:{}),...(preset?{preset}:{})},flags:{preview,original}}},this.trackID,before);
            if (generation!==this.generation) return;
            if (!reply || epoch!==this.epoch?.()) { this.conflict(); return; }
            const result=payload<RemotePitchAnalysis>(reply.payload); validateModel(result,'RemotePitchAnalysis');
            if(result.error) {this.conflict();this.message(result.error);return;}
            const current=this.track()?.realtimeEffects.find(e=>(e as RealtimeEffect).kind==='pitchCorrection') as RealtimeEffect|undefined;
            const unchanged=operation!=='pitch.targets'||!!current&&sameEffectSettings(current,{...current,pitchEdits:replacePitchScope(current.pitchEdits??[],result.regions,this.scope)});
            const next=ownAcceptedTrackRevision(before,this.mirror.snapshot?.trackRevisions[this.trackID],reply,unchanged);
            if(next===undefined || !this.track()) { this.conflict(); return; }
            this.lastTrackRevision=next; this.sourceEpoch=epoch; this.blocked=false; this.auditioning=preview;
            if (result.isOutput) this.after=result.points;
            else { this.before=result.points; this.after=[]; this.regions=result.regions; this.draft=result.targetsAreDraft; this.effectID=current?.id??this.effectID; }
            if(reload) { this.parameters={...Object.fromEntries(this.meta()!.parameters.map(p=>[p.id,p.defaultValue])),...current?.parameters}; this.fit(); }
            this.message(result.error??`${result.points.filter(p=>p.frequency>0&&p.confidence>=0.45).length} voiced measurements · ${pitchPointCount(this.regions)} targets${this.draft?' · draft':''}`);
        } catch(error) { if(generation===this.generation) {this.conflict();this.message(error instanceof Error?error.message:'Invalid pitch analysis.');} }
        finally { if(generation===this.generation) {this.busy=false;this.renderToolbar();this.renderControls();this.dirty=true;} }
    }
    private async commit(regions:PitchEditRegion[],name:string,gestureID?:string,only?:boolean,guard?:TrackGestureRevision) {
        const track=this.track(); if(!track || !this.enabled() || this.busy) return false;
        if(!this.ready() || guard&&!guard.ready()) {this.conflict();return false;}
        this.regions=regions;
        const generation=this.generation,before=this.lastTrackRevision,draftAtStart=this.regions,parameters={...this.parameters},current=this.effect();
        const effect:RealtimeEffect={...(current??{id:randomUUID().toUpperCase(),kind:'pitchCorrection',amount:.5,enabled:true,automation:[],parameterAutomation:{},bakedRanges:[],pitchEditsOnly:true}),parameters,pitchEdits:replacePitchScope(current?.pitchEdits??[],regions,this.scope),pitchEditsOnly:only??current?.pitchEditsOnly??true};
        const effects=track.realtimeEffects.map(e=>(e as RealtimeEffect).id===effect.id?effect:e); if(!current) effects.unshift(effect);
        this.draft=true;this.writing++;
        try {
            const reply=await this.send({type:'updateTrackControls',controls:{...trackControls(track),realtimeEffects:effects},name,gestureID},track.id,before);
            if(generation!==this.generation) return false;
            const observed=this.track()?.realtimeEffects.find(e=>(e as RealtimeEffect).id===effect.id) as RealtimeEffect|undefined;
            const unchanged=sameEffectSettings(observed,effect),next=ownAcceptedTrackRevision(before,this.mirror.snapshot?.trackRevisions[track.id],reply,unchanged);
            if(!reply || this.sourceEpoch!==this.epoch?.() || next===undefined || !this.track() || guard&&!guard.accepted(reply,unchanged)) {this.conflict();return false;}
            this.effectID=effect.id;this.lastTrackRevision=next;
            if(this.regions===draftAtStart && Object.keys({...parameters,...this.parameters}).every(k=>Math.fround(parameters[k]!)===Math.fround(this.parameters[k]!))) {this.regions=regions;this.draft=false;}
            this.after=[];this.revision=this.mirror.snapshot?.revision??this.revision;this.updatePoint();this.dirty=true;return true;
        } catch(error) { if(generation===this.generation) this.conflict(); return false; }
        finally {if(generation===this.generation) {this.writing--;this.renderToolbar();}}
    }
    private writer(name:string) {
        const id=randomUUID().toUpperCase(),generation=this.generation;
        const guard=new TrackGestureRevision(this.trackID,()=>generation===this.generation?this.trackID:undefined,id=>this.mirror.snapshot?.trackRevisions[id],this.epoch);
        return new GestureWriter<PitchEditRegion[]>(async regions=>generation===this.generation&&await this.commit(regions,name,id,undefined,guard));
    }
    private x(frame: number) { return 54 + (frame - this.scope[0]) * this.scale - this.offset; }
    private frameAt(x: number) { return Math.round(Math.max(this.scope[0], Math.min(this.scope[1], this.scope[0] + (x - 54 + this.offset) / this.scale))); }
    private y(note: number) { return 26 + (this.top - note) * this.semitonePixels; }
    private noteAt(y: number) { const value = Math.max(0, Math.min(95, this.top - (y - 26) / this.semitonePixels)); return this.snap.checked ? Math.round(value) : value; }
    private find(x: number, y: number) { let best: PitchEditPoint | undefined, distance = 100; for (const region of this.regions)
        for (const p of region.points ?? []) {
            const d = (this.x(p.frame) - x) ** 2 + (this.y(p.note) - y) ** 2;
            if (d < distance) {
                best = p;
                distance = d;
            }
        } return best; }
    private down(e: PointerEvent) { if (e.button !== 0 || this.busy || !this.enabled())
        return; const point = this.find(e.offsetX, e.offsetY); this.selected = point?.id; this.canvas.focus(); if (point?.id) {
        this.drag = { id: point.id, writer: this.writer('Move pitch targets') };
        this.canvas.setPointerCapture(e.pointerId);
    } this.updatePoint(); this.dirty = true; }
    private move(e: PointerEvent) { if (!this.drag)
        return; this.regions = editPitchPoint(this.regions, this.drag.id, this.frameAt(e.offsetX), this.noteAt(e.offsetY)); this.draft=true; this.drag.writer.offer(structuredClone(this.regions)); this.updatePoint(); this.dirty = true; }
    private addPoint(e: MouseEvent) { if (this.busy || !this.enabled())
        return; if (pitchPointCount(this.regions) >= 100000) {
        this.message('Pitch target point limit reached.');
        return;
    } const added = addPitchPoint(this.regions, this.frameAt(e.offsetX), this.noteAt(e.offsetY)); if (!added) {
        this.message('Add points inside a voiced target region. Analyze the audio first.');
        return;
    } this.selected = added.point.id; this.regions = added.regions; this.draft=true; this.writer('Add pitch target').offer(this.regions); this.updatePoint(); this.dirty = true; }
    private point() { return this.regions.flatMap(r => r.points ?? []).find(p => p.id === this.selected); }
    private updatePoint() { const p = this.point(); this.pointPosition.disabled = this.pointNote.disabled = this.curve.disabled = !p || this.busy || !this.enabled(); if (p) {
        this.pointPosition.value = formatPosition(p.frame, this.mirror.snapshot!.session.timing, this.mirror.sampleRate, this.mirror.view.timebase);
        this.pointNote.value = p.note.toFixed(2);
        this.curve.value = p.curve ?? 'linear';
        this.pointLabel.textContent = `${noteName(Math.round(p.note))} · ${((p.note - Math.round(p.note)) * 100).toFixed(1)} cents`;
    }
    else
        this.pointLabel.textContent = 'Select a target point.'; }
    private editNumeric() { const point = this.point(); if (!point?.id || this.busy || !this.enabled())
        return; try {
        const frame = parsePosition(this.pointPosition.value, this.mirror.snapshot!.session.timing, this.mirror.sampleRate, this.mirror.view.timebase), note = Number(this.pointNote.value);
        if (!Number.isFinite(note) || note < 0 || note > 95)
            throw Error('Target note must be between MIDI 0 and 95.');
        this.regions = editPitchPoint(this.regions, point.id, frame, note, this.curve.value as AutomationCurve);
        this.draft=true; this.writer('Edit pitch target').offer(this.regions);
        this.dirty = true;
        this.updatePoint();
    }
    catch (error) {
        this.message(error instanceof Error ? error.message : 'Invalid target.');
    } }
    private async removePoint() { if (!this.selected || !this.enabled() || this.busy)
        return; const regions = this.regions.map(r => ({ ...r, points: r.points?.filter(p => p.id !== this.selected) })).filter(r => r.points?.length); if (await this.commit(regions, 'Delete pitch target')) {
        this.selected = undefined;
        this.updatePoint();
    } }
    private key(e: KeyboardEvent) { if (!acceptWorkspaceShortcut(e))
        return; if (e.code === 'Space') {
        e.preventDefault();
        if (this.canOperate()) {
            if (this.auditioning) {
                void this.stopAudition();
            }
            else
                void this.analyze('pitch.render', true);
        }
    }
    else if (isItemNavigation(e.key) && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault();
        const points=this.regions.flatMap(r=>r.points??[]),point=navigateTimelineItem(points,this.point(),e.key,p=>p.frame);
        if(point?.id){this.selected=point.id;this.updatePoint();const x=this.x(point.frame),y=this.y(point.note);if(x<70||x>this.width-20)this.offset=Math.max(0,(point.frame-this.scope[0])*this.scale-(this.width-54)/2);if(y<40)this.top=point.note+2;else if(y>this.height-20)this.top=point.note+(this.height-50)/this.semitonePixels;this.dirty=true;this.message(`Selected ${noteName(Math.round(point.note))} at ${this.pointPosition.value}. Arrow keys adjust this target.`);}
    }
    else if (e.key === 'Delete' || e.key === 'Backspace') {
        e.preventDefault();
        void this.removePoint();
    }
    else if (e.key.startsWith('Arrow')) {
        const p = this.point();
        if (!p?.id || !this.enabled() || this.busy)
            return;
        e.preventDefault();
        const frame = p.frame + (e.key === 'ArrowLeft' ? -1 : e.key === 'ArrowRight' ? 1 : 0) * Math.round(this.mirror.sampleRate * (e.shiftKey ? 0.1 : 0.01)), note = p.note + (e.key === 'ArrowUp' ? 1 : e.key === 'ArrowDown' ? -1 : 0) * (e.shiftKey ? 1 : 0.01);
        this.regions = editPitchPoint(this.regions, p.id, frame, note);
        if (!e.repeat || !this.keyboardWriter)
            this.keyboardWriter = this.writer('Nudge pitch target');
        this.draft=true; this.keyboardWriter.offer(structuredClone(this.regions));
        this.updatePoint();
        this.dirty = true;
    } }
    private zoom(factor: number, anchor = this.width / 2) { const next = Math.min(2, Math.max(0.00001, this.scale * factor)); this.offset = zoomAround(this.scale, next, this.offset, anchor - 54); this.scale = next; this.dirty = true; }
    private fit() { this.offset = 0; this.scale = Math.max(0.00001, (this.width - 70) / Math.max(1, this.scope[1] - this.scope[0])); const notes = this.before.filter(p => p.frequency > 0 && p.confidence >= 0.45).map(p => frequencyNote(p.frequency, this.parameters.reference ?? 440)).filter(Number.isFinite).sort((a, b) => a - b); if (notes.length) {
        const low = notes[Math.floor(notes.length * 0.03)]!, high = notes[Math.floor(notes.length * 0.97)]!;
        this.top = Math.min(98, Math.ceil(high) + 3);
        this.semitonePixels = Math.min(48, Math.max(5, (this.height - 50) / Math.max(12, high - low + 6)));
    } this.dirty = true; }
    private trace(points: RemotePitchPoint[], color: string) { const c = this.context, rate = this.parameters.reference ?? 440; let drawn = false, lastX = -Infinity, lastFrame = -Infinity; c.strokeStyle = color; c.lineWidth = 1.3; c.beginPath(); for (const p of points) {
        const x = this.x(p.frame);
        if (x < 54 || x > this.width)
            continue;
        if (p.frequency <= 0 || p.confidence < 0.45) {
            drawn = false;
            continue;
        }
        if (x - lastX < 0.75)
            continue;
        const y = this.y(frequencyNote(p.frequency, rate));
        if (!drawn || p.frame - lastFrame > this.mirror.sampleRate * 0.03)
            c.moveTo(x, y);
        else
            c.lineTo(x, y);
        drawn = true;
        lastX = x;
        lastFrame = p.frame;
    } c.stroke(); }
    private draw() { const c = this.context, w = this.width, h = this.height; c.fillStyle = '#111922'; c.fillRect(0, 0, w, h); c.font = '10px system-ui'; for (let note = Math.ceil(this.top); note >= 0; note--) {
        const y = this.y(note);
        if (y > h)
            break;
        if (y < 26)
            continue;
        c.strokeStyle = note % 12 === 0 ? '#596c8155' : '#35445566';
        c.beginPath();
        c.moveTo(54, y);
        c.lineTo(w, y);
        c.stroke();
        c.fillStyle = '#afbed0';
        c.fillText(noteName(note), 6, y + 4);
    } const duration = (w - 54) / this.scale, interval = 10 ** Math.floor(Math.log10(Math.max(1, duration / this.mirror.sampleRate / 6))) * this.mirror.sampleRate; for (let frame = Math.ceil((this.scope[0] + this.offset / this.scale) / interval) * interval; frame < this.scope[1] && this.x(frame) < w; frame += interval) {
        const x = this.x(frame);
        c.strokeStyle = '#40506666';
        c.beginPath();
        c.moveTo(x, 26);
        c.lineTo(x, h);
        c.stroke();
        c.fillStyle = '#96a8bd';
        c.fillText((frame / this.mirror.sampleRate).toFixed(1) + ' s', x + 3, 17);
    } c.save(); c.beginPath(); c.rect(54, 26, w - 54, h - 26); c.clip(); this.trace(this.before, '#8098ae'); this.trace(this.after, '#71d0b1'); for (const region of this.regions) {
        const left = this.x(region.startFrame), right = this.x(region.endFrame);
        if (right < 54 || left > w)
            continue;
        c.fillStyle = this.draft ? '#d6b66d0d' : '#89b8ed0d';
        c.fillRect(left, 26, right - left, h - 26);
        const points = region.points ?? [];
        if (points.length) {
            const curve = points.map(p => ({ ...p, value: p.note })) as Point[], sample = preparedAutomation(curve, points[0]!.note);
            c.strokeStyle = '#edc774';
            c.lineWidth = 2;
            c.beginPath();
            for (let x = Math.max(54, left); x <= Math.min(w, right); x += 2) {
                const frame = this.frameAt(x), y = this.y(sample(frame));
                if (x === Math.max(54, left))
                    c.moveTo(x, y);
                else
                    c.lineTo(x, y);
            }
            c.stroke();
        }
        for (const point of points) {
            const x = this.x(point.frame), y = this.y(point.note);
            if (x < 48 || x > w + 6 || y < 20 || y > h + 6)
                continue;
            c.fillStyle = point.id === this.selected ? '#fff0ba' : '#e4c17c';
            c.beginPath();
            c.arc(x, y, point.id === this.selected ? 4.5 : 3, 0, Math.PI * 2);
            c.fill();
        }
    } c.restore(); }
    private tick() { if (this.dialog.open && this.dirty) {
        this.draw();
        this.dirty = false;
    } this.raf = requestAnimationFrame(() => this.tick()); }
    refresh() { if (!this.dialog.open)
        return; this.renderToolbar(); for (const input of this.controls.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>('input,select,button'))
        input.disabled = this.busy || !this.enabled(); this.updatePoint(); if (!this.writing && !this.busy && !this.ready()) this.conflict(); if (!this.track()) {
        this.message('The source track no longer exists. Close this editor.');
    } }
    private async bake() {
        if(!this.effectID || this.busy || this.writing || !this.enabled() || this.draft) return;
        if(!this.ready()) {this.conflict();return;}
        if(!confirm('Apply the complete live insert chain and its automation to audio? Undo and clip history retain the source.')) return;
        const generation=this.generation,before=this.lastTrackRevision; this.busy=true;this.renderToolbar();
        try {
            const reply=await this.send({type:'editor',intent:{operation:'pitch.apply',context:{trackID:this.trackID,effectID:this.effectID,cursorFrame:this.scope[0],clipIDs:[]},integers:{},numbers:{},strings:{trackID:this.trackID,effectID:this.effectID},flags:{}}},this.trackID,before);
            if(generation!==this.generation) return;
            if(!reply || this.sourceEpoch!==this.epoch?.() || ownAcceptedTrackRevision(before,this.mirror.snapshot?.trackRevisions[this.trackID],reply)===undefined) {this.conflict();return;}
            await this.close(); if(this.generation===generation+1) this.message('Pitch and live effects applied to audio.');
        } finally {if(generation===this.generation) {this.busy=false;this.renderToolbar();}}
    }
    private async stopAudition() {
        const generation=this.generation;
        if (this.canOperate()) await this.send({ type: 'editor', intent: { operation: 'audition.stop', context: { cursorFrame: 0, clipIDs: [] }, integers: {}, numbers: {}, strings: { kind: 'pitch' }, flags: {} } });
        if(generation===this.generation) this.auditioning = false;
    }
    async close() { const generation=++this.generation; this.drag?.writer.cancel(); this.keyboardWriter?.cancel(); this.drag = undefined; await this.stopAudition(); if(generation===this.generation) this.dialog.close(); }
}
