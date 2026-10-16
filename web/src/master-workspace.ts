import { openWorkspaceDialog } from './input-focus.js';
import { inputScale, sameEffectSettings } from './effect-models.js';
import { type Command, type SessionMirror } from './model.js';
import type { MasteringAnalysis, RealtimeEffect } from './generated/models.js';
import { type EffectMetadata } from './effects.js';
import { payload } from './transfers.js';
import { encodeBase64 } from './sdk.js';
import { validateModel } from './validate.js';
import { GestureWriter } from './automation.js';
import { formatPosition } from './timing.js';
import { ownAcceptedTrackRevision } from './gesture-revision.js';
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, text?: string) => { const e = document.createElement(tag); if (text)
    e.textContent = text; return e; };
type Sender = (command: Command, trackID?: string, expectedTrackRevision?:number) => Promise<{
    payload?: unknown; revision?:number;
} | undefined>;
export class MasterWorkspace {
    private dialog: HTMLDialogElement;
    private toolbar: HTMLElement;
    private controls: HTMLElement;
    private metrics: HTMLElement;
    private chart: HTMLCanvasElement;
    private readout: HTMLElement;
    private rack: HTMLElement;
    private status: HTMLElement;
    private context: CanvasRenderingContext2D;
    private width = 800;
    private height = 210;
    private target = -14;
    private intensity = 0.65;
    private proposed: RealtimeEffect[] = [];
    private report?: MasteringAnalysis;
    private busy = false;
    private generation = 0;
    private sourceToken = '';
    private sourceMasterID?:string;
    private sourceMasterRevision?:number;
    private sourceEpoch?:number;
    private proposalReady=false;
    private proposalSourceToken='';
    private proposalSerial=0;
    private manualVersion=0;
    private writer: GestureWriter<{
        target: number;
        intensity: number; source:string; generation:number; serial:number; manualVersion:number;
    }>;
    private resize: ResizeObserver;
    constructor(private mirror: SessionMirror, private send: Sender, private metadata: () => EffectMetadata[], private enabled: () => boolean, private canOperate: () => boolean, private done: () => void, private epoch?:()=>number) {
        this.dialog = el('dialog');
        this.dialog.className = 'master-dialog';
        this.dialog.setAttribute('aria-label','Master mix analysis and processing');
        this.toolbar = el('div');
        this.toolbar.className = 'workspace-toolbar';
        this.metrics = el('div');
        this.metrics.className = 'master-metrics';
        this.controls = el('div');
        this.controls.className = 'master-controls';
        this.chart = el('canvas');
        this.chart.className = 'master-chart';
        this.chart.setAttribute('role', 'img');
        this.readout = el('div');
        this.readout.className = 'section-readout';
        this.readout.textContent = 'Measured sections · loudness and true peak';
        this.rack = el('div');
        this.rack.className = 'master-proposal';
        this.status = el('div');
        this.status.className = 'workspace-status';
        this.status.setAttribute('role', 'status');
        const ctx = this.chart.getContext('2d');
        if (!ctx)
            throw Error('Master chart unavailable.');
        this.context = ctx;
        this.dialog.append(this.toolbar, this.metrics, this.chart, this.readout, this.controls, this.rack, this.status);
        document.body.append(this.dialog);
        this.dialog.oncancel = e => { e.preventDefault(); void this.close(); };
        this.chart.onpointermove = e => this.sectionAt(e.offsetX);
        this.resize = new ResizeObserver(() => { this.width = this.chart.clientWidth; const dpr = Math.min(devicePixelRatio, 2); this.chart.width = this.width * dpr; this.chart.height = this.height * dpr; this.context.setTransform(dpr, 0, 0, dpr, 0, 0); this.draw(); });
        this.resize.observe(this.chart);
        this.writer = new GestureWriter(async (values) => await this.proposal(values.target, values.intensity,values.source,values.generation,values.serial,values.manualVersion));
    }
    private token() {
        const snapshot=this.mirror.snapshot;if(!snapshot) return '';
        const master=snapshot.session.masterTrack;
        return `${this.epoch?.()??''}|${snapshot.session.tracks.map(t=>`${t.id}:${snapshot.trackRevisions[t.id]??0}`).join('|')}|master:${master?.id??''}:${master?snapshot.trackRevisions[master.id]??0:''}`;
    }
    private bindSource() {
        this.sourceToken=this.token();this.sourceEpoch=this.epoch?.();const track=this.mirror.snapshot?.session.masterTrack;
        this.sourceMasterID=track?.id;this.sourceMasterRevision=track?this.mirror.snapshot?.trackRevisions[track.id]:undefined;
    }
    private stale() { return !this.sourceToken || this.sourceToken!==this.token() || this.sourceEpoch!==this.epoch?.(); }
    private button(label:string,action:()=>void,operate=false) {const b=el('button',label);b.disabled=this.busy||!this.enabled()||operate&&!this.canOperate();b.onclick=action;return b;}
    open() {
        if(!this.mirror.snapshot) return;
        this.generation++;this.busy=false;this.proposed=[];this.proposalReady=false;this.proposalSourceToken='';this.proposalSerial=0;this.manualVersion=0;
        this.report=this.mirror.snapshot.session.masteringAnalysis;this.bindSource();this.writer.cancel();
        this.writer=new GestureWriter(async values=>await this.proposal(values.target,values.intensity,values.source,values.generation,values.serial,values.manualVersion));
        this.render();openWorkspaceDialog(this.dialog);
        if(this.report) this.queueProposal();
        else this.status.textContent='Analyze the mix to review loudness, peaks, tonal balance and a bounded processing proposal.';
    }
    private render() {
        this.toolbar.replaceChildren();
        this.toolbar.append(el('strong', 'Master the mix'), this.button('Analyze mix', () => void this.analyze()), this.button('Play mix', () => void this.send({ type: 'play' }), true));
        {
            const cancel = el('button', 'Cancel analysis');
            cancel.disabled = !this.busy;
            cancel.onclick = () => void this.send({ type: 'editor', intent: { operation: 'jobs.cancel', context: { cursorFrame: 0, clipIDs: [] }, numbers: {}, integers: {}, strings: {}, flags: {} } });
            this.toolbar.append(cancel);
        }
        const apply = this.button('Use proposal', () => void this.apply());
        apply.disabled = this.busy || !this.enabled() || !this.report || this.report.truePeakDB < -70 || this.stale() || !this.proposed.length || !this.proposalReady;
        this.toolbar.append(apply);
        const close = el('button', 'Done');
        close.onclick = () => void this.close();
        this.toolbar.append(close);
        this.metrics.replaceChildren();
        for (const [name, value] of [['Integrated', this.report ? `${this.report.integratedLUFS.toFixed(1)} LUFS` : '—'], ['True peak', this.report ? `${this.report.truePeakDB.toFixed(1)} dBTP` : '—'], ['Crest', this.report ? `${this.report.crestDB.toFixed(1)} dB` : '—'], ['Bass / Air', this.report ? `${(this.report.bassFraction * 100).toFixed(0)}% / ${(this.report.airFraction * 100).toFixed(0)}%` : '—']]) {
            const metric = el('div');
            metric.append(el('span', name), el('strong', value));
            this.metrics.append(metric);
        }
        this.controls.replaceChildren();
        const target = el('label', 'Loudness aim'), select = el('select');
        select.setAttribute('aria-label', 'Master loudness aim');
        for (const [label, value] of [['Dynamic · −18 LUFS', -18], ['Balanced · −16 LUFS', -16], ['Forward · −14 LUFS', -14]] as const)
            select.add(new Option(label, String(value)));
        select.value = String(this.target);
        select.disabled = this.busy || !this.report || this.stale();
        select.onchange = () => { this.target = Number(select.value); this.queueProposal(); };
        target.append(select);
        const amount = el('label', 'Proposal amount'), input = el('input');
        input.type = 'range';
        input.min = '0';
        input.max = '1';
        input.step = '0.01';
        input.value = String(this.intensity);
        input.disabled = this.busy || !this.report || this.stale();
        input.setAttribute('aria-label', 'Mastering proposal amount');
        const percent = el('output', Math.round(this.intensity * 100) + '%');
        input.oninput = () => { this.intensity = Number(input.value); percent.textContent = Math.round(this.intensity * 100) + '%'; };
        input.onchange = () => this.queueProposal();
        amount.append(input, percent);
        this.controls.append(target, amount);
        this.renderRack();
        this.draw();
    }
    private queueProposal() {
        if(this.busy||!this.report||this.stale()) {this.status.textContent='The mix or Master rack changed. Your proposal is kept here. Analyze again to review the current mix.';return;}
        this.proposalReady=false;this.status.textContent='Preparing the native proposal…';
        this.writer.offer({target:this.target,intensity:this.intensity,source:this.sourceToken,generation:this.generation,serial:++this.proposalSerial,manualVersion:this.manualVersion});
        this.render();
    }
    private async analyze() {
        if(this.busy||!this.enabled()) return;
        if(this.proposed.length&&!confirm('Analyze the current mix and replace the proposal kept in this editor?')) return;
        this.writer.cancel();const generation=++this.generation,source=this.token();this.busy=true;this.proposalReady=false;this.render();
        this.status.textContent='Measuring the full mix on the hardware server…';
        try {
            const reply=await this.send({type:'editor',intent:{operation:'master.analyze',context:{cursorFrame:0,clipIDs:[]},integers:{},numbers:{},strings:{},flags:{}}});
            if(generation!==this.generation) return;
            const report=this.mirror.snapshot?.session.masteringAnalysis;
            if(!reply||source!==this.token()||!report) {this.status.textContent='Analysis could not be confirmed for this mix. Your proposal is retained; analyze again to review the current source.';return;}
            this.report=report;this.bindSource();this.proposed=[];this.proposalSourceToken='';this.manualVersion=0;this.busy=false;this.queueProposal();
        } catch(error) {if(generation===this.generation) {this.proposalReady=false;this.status.textContent=error instanceof Error?error.message:'The operation could not be confirmed. Your draft is retained.';}}
        finally {if(generation===this.generation) {this.busy=false;this.render();}}
    }
    private async proposal(target:number,intensity:number,source=this.sourceToken,generation=this.generation,serial=this.proposalSerial,manualVersion=this.manualVersion) {
        if(!this.report||generation!==this.generation||source!==this.sourceToken||source!==this.token()) return false;
        let reply:Awaited<ReturnType<Sender>>;
        try {reply=await this.send({type:'editor',intent:{operation:'master.proposal',context:{cursorFrame:0,clipIDs:[]},integers:{},numbers:{targetLUFS:target,intensity},strings:{},flags:{}}});}
        catch(error) {if(generation===this.generation&&serial===this.proposalSerial) {this.proposalReady=false;this.status.textContent=error instanceof Error?error.message:'The proposal could not be confirmed.';this.render();}return false;}
        if(generation!==this.generation||serial!==this.proposalSerial) return false;
        if(!reply||source!==this.token()||source!==this.sourceToken) {this.proposalReady=false;this.status.textContent='The mix or Master rack changed, or this proposal could not be confirmed. Your draft is retained. Analyze again before applying.';this.render();return false;}
        if(target!==this.target||intensity!==this.intensity) {this.proposalReady=false;this.status.textContent='Your newer loudness and amount settings are retained. Finish changing them to request an updated proposal.';this.render();return false;}
        if(manualVersion!==this.manualVersion) {this.proposalReady=this.proposalSourceToken===source&&!this.stale();this.status.textContent='Your newer manual settings are retained. Use this proposal, or request a new one when you are ready to replace them.';this.render();return false;}
        try {
            const proposed=payload<RealtimeEffect[]>(reply.payload);validateModel(proposed,'[RealtimeEffect]');if(proposed.length>64) throw Error('Invalid Master proposal.');
            this.proposed=proposed;this.proposalSourceToken=source;this.proposalReady=true;
            this.status.textContent='Native proposal · section automation and every parameter remain editable. Applying replaces the Master insert chain; Undo retains the previous rack.';this.render();return true;
        } catch(error) {this.proposalReady=false;this.status.textContent=error instanceof Error?error.message:'Invalid mastering proposal.';this.render();return false;}
    }
    private renderRack() { this.rack.replaceChildren(); for (const effect of this.proposed) {
        const meta = this.metadata().find(m => m.id === effect.kind);
        if (!meta)
            continue;
        const card = el('details');
        card.className = 'proposal-effect';
        const summary = el('summary'), enabled = el('input');
        enabled.type = 'checkbox';
        enabled.checked = effect.enabled ?? true;
        enabled.disabled = this.busy || !this.enabled() || this.stale();
        enabled.onclick = e => e.stopPropagation();
        enabled.onchange = () => {effect.enabled=enabled.checked;this.manualVersion++;};
        summary.append(enabled, el('strong', meta.name));
        card.append(summary);
        const description = el('p', this.reason(effect));
        description.className = 'hardware-note';
        card.append(description);
        const form = el('div');
        form.className = 'proposal-parameters';
        for (const p of meta.parameters) {
            const label = el('label', p.name), input = p.choices ? el('select') : el('input');
            if (input instanceof HTMLSelectElement)
                p.choices!.forEach((value, index) => input.add(new Option(value, String(p.minimum+index))));
            else {
                input.type = 'number';
                input.min = String(p.minimum*inputScale(p));
                input.max = String(p.maximum*inputScale(p));
                input.step = p.integer ? '1' : String(Math.max(0.001, (p.maximum - p.minimum)*inputScale(p) / 1000));
            }
            input.value = String(Number(((effect.parameters?.[p.id] ?? p.defaultValue)*inputScale(p)).toPrecision(6)));
            input.disabled = this.busy || !this.enabled() || this.stale();
            input.setAttribute('aria-label', `${meta.name} proposal ${p.name}`);
            input.onchange = () => { const value = Number(input.value)/(p.choices?1:inputScale(p)); if (!Number.isFinite(value) || value < p.minimum || value > p.maximum) {
                this.status.textContent = `${p.name} is outside its range.`;
                return;
            } effect.parameters = { ...effect.parameters, [p.id]: p.integer||p.choices ? Math.round(value) : value };this.manualVersion++; };
            label.append(input);
            if (p.unit)
                label.append(el('span', p.unit));
            form.append(label);
        }
        card.append(form);
        for (const [key, points] of Object.entries(effect.parameterAutomation ?? {})) {
            card.append(el('p', `${meta.parameters.find(p => p.id === key)?.name ?? key} · ${points.length} smooth section points. Open Master automation after applying to edit the curve.`));
        }
        this.rack.append(card);
    } }
    private reason(effect: RealtimeEffect) { switch (effect.kind) {
        case 'equalizer': return effect.enabled ? 'Small broad shelf cuts from measured energy balance.' : 'Broad tonal balance needs no suggested shelf correction.';
        case 'resonance': return effect.enabled ? 'Bounded spectral reduction follows a measured section curve.' : 'No strong spectral peaks detected; available for manual use.';
        case 'warmth': return 'Optional tape color at a low blend. Enable if it suits the song.';
        case 'limiter': return '−1 dBTP ceiling. Peak-aware input automation preserves section dynamics; loudness aim is a reference, not a guaranteed result.';
        default: return 'Native live processing.';
    } }
    private draw() { const c = this.context, w = this.width, h = this.height; c.fillStyle = '#111922'; c.fillRect(0, 0, w, h); c.font = '11px system-ui'; const report = this.report; if (!report)
        return; const left = 45, width = w - left - 12; for (const level of [-48, -36, -24, -12, 0]) {
        const y = 20 - level / 60 * (h - 45);
        c.strokeStyle = '#354353';
        c.beginPath();
        c.moveTo(left, y);
        c.lineTo(w, y);
        c.stroke();
        c.fillStyle = '#9dacc0';
        c.fillText(String(level), 8, y + 4);
    } for (const [key, color] of [['loudness', '#dfb66b'], ['truePeak', '#77c8b3']] as const) {
        c.strokeStyle = color;
        c.lineWidth = 2;
        c.beginPath();
        let first = true, last = -Infinity;
        for (const section of report.sections) {
            const x = left + (section.start + section.end) / 2 / Math.max(1, report.analyzedFrames) * width;
            if (x - last < 0.75)
                continue;
            const y = 20 + Math.min(60, Math.max(0, -section[key])) / 60 * (h - 45);
            if (first)
                c.moveTo(x, y);
            else
                c.lineTo(x, y);
            first = false;
            last = x;
        }
        c.stroke();
    } c.fillStyle = '#dfb66b'; c.fillText('Section loudness', left, 13); c.fillStyle = '#77c8b3'; c.fillText('True peak', left + 150, 13); c.fillStyle = '#a6b4c6'; c.fillText('0:00', left, h - 3); c.fillText(`${(report.analyzedFrames / this.mirror.sampleRate).toFixed(1)} s`, Math.max(left, w - 70), h - 3); this.chart.setAttribute('aria-label', `Measured section loudness and true peak over ${report.sections.length} sections, ${(report.analyzedFrames / this.mirror.sampleRate).toFixed(1)} seconds. Integrated ${report.integratedLUFS.toFixed(1)} LUFS, true peak ${report.truePeakDB.toFixed(1)} dBTP.`); }
    private sectionAt(x: number) { const report = this.report; if (!report)
        return; const frame = (x - 45) / Math.max(1, this.width - 57) * report.analyzedFrames, section = report.sections.find(s => frame >= s.start && frame < s.end); if (section) {
        const timing = this.mirror.snapshot!.session.timing, rate = this.mirror.sampleRate;
        this.readout.textContent = `${formatPosition(section.start, timing, rate, 'time')} – ${formatPosition(section.end, timing, rate, 'time')} · ${section.loudness.toFixed(1)} LUFS · ${section.truePeak.toFixed(1)} dBTP · resonance ${section.resonance.toFixed(2)} dB`;
    } }
    private async apply() {
        const track=this.mirror.snapshot?.session.masterTrack;
        if(!track||!this.enabled()||this.busy||this.stale()||!this.proposed.length||!this.proposalReady||track.id!==this.sourceMasterID||this.sourceMasterRevision===undefined) return;
        const generation=this.generation,before=this.sourceMasterRevision,source=this.sourceToken,proposed=structuredClone(this.proposed);
        this.writer.cancel();this.busy=true;this.render();
        try {
            const reply=await this.send({type:'editor',intent:{operation:'master.applyProposal',context:{trackID:track.id,cursorFrame:0,clipIDs:[]},integers:{},numbers:{},strings:{},flags:{},payload:encodeBase64(new TextEncoder().encode(JSON.stringify(proposed)))}},track.id,before);
            if(generation!==this.generation) return;
            const current=this.mirror.snapshot?.session.masterTrack,unchanged=current?.id===track.id&&current.realtimeEffects.length===proposed.length&&current.realtimeEffects.every((e,i)=>sameEffectSettings(e as RealtimeEffect,proposed[i]!));
            const next=ownAcceptedTrackRevision(before,this.mirror.snapshot?.trackRevisions[track.id],reply,unchanged);
            // Applying changes only Master: arrangement source and connection must still match the captured source.
            const expectedToken=current?.id===track.id&&next!==undefined?source.replace(/\|master:[^|]+$/,`|master:${track.id}:${next}`):undefined;
            if(!reply||next===undefined||current?.id!==track.id||this.sourceEpoch!==this.epoch?.()||this.token()!==expectedToken) {this.proposalReady=false;this.status.textContent='The Master edit could not be confirmed against this source. Your proposal is retained; review the current mix before applying again.';return;}
            this.dialog.close();this.done();
        } catch(error) {if(generation===this.generation) {this.proposalReady=false;this.status.textContent=error instanceof Error?error.message:'The operation could not be confirmed. Your draft is retained.';}}
        finally {if(generation===this.generation) {this.busy=false;this.render();}}
    }
    refresh() { if (!this.dialog.open)
        return; if (this.report && this.stale()) {
        this.proposalReady=false;this.status.textContent = 'The mix or Master rack changed. Your proposal is kept here. Analyze again before applying.';
        this.render();
    } }
    async close() { this.generation++; this.writer.cancel(); this.dialog.close(); }
}
