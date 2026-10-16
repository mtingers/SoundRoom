import {randomUUID} from './crypto.js';
import { acceptWorkspaceShortcut, isItemNavigation, navigateTimelineItem } from './input-focus.js';
import { sameWriteTarget as sameLiveTarget } from './automation-write.js';
import { AutomationTouchBindings,TouchControlBinding,type TouchGesture } from './automation-touch.js';
import type {RemoteAutomationWriteTarget} from './generated/models.js';
import { TrackGestureRevision } from './gesture-revision.js';
import { type Track, type Command, type Timing } from './model.js';
import { type Insert, type EffectMetadata, trackControls, parameterValue, parameterNormalized } from './effects.js';
import type { RealtimeEffect } from './generated/models.js';
import { editableEffect, inputScale } from './effect-models.js';
import { snapFrame, formatPosition, parsePosition } from './timing.js';
export type Curve = 'linear' | 'smooth' | 'easeIn' | 'easeOut' | 'hold';
export interface Point {
    frame: number;
    value: number;
    curve: Curve;
    curveStart: number;
    curveEnd: number;
}
export interface Target {
    id: string;
    name: string;
    effectID?: string;
    parameter: string;
}
export function curveFraction(x: number, curve: Curve) { const f = Math.min(1, Math.max(0, x)); switch (curve) {
    case 'smooth': return f * f * (3 - 2 * f);
    case 'easeIn': return f * f;
    case 'easeOut': return 1 - (1 - f) * (1 - f);
    case 'hold': return f < 1 ? 0 : 1;
    default: return f;
} }
export function automationValue(points: Point[], frame: number, baseline: number, decibels = false) { let lower: Point | undefined, upper: Point | undefined; for (const p of points) {
    if (p.frame <= frame && (!lower || p.frame >= lower.frame))
        lower = p;
    else if (p.frame > frame && (!upper || p.frame < upper.frame))
        upper = p;
} if (!lower)
    return baseline; if (!upper)
    return lower.value; let f = (frame - lower.frame) / (upper.frame - lower.frame); if (lower.curve !== 'linear') {
    const start = curveFraction(lower.curveStart ?? 0, lower.curve), end = curveFraction(lower.curveEnd ?? 1, lower.curve), raw = curveFraction((lower.curveStart ?? 0) + ((lower.curveEnd ?? 1) - (lower.curveStart ?? 0)) * f, lower.curve);
    f = Math.abs(end - start) > 0.000001 ? (raw - start) / (end - start) : f;
} if (decibels && f > 0 && f < 1) {
    const a = 20 * Math.log10(Math.max(0.0001, lower.value * 1.5)), b = 20 * Math.log10(Math.max(0.0001, upper.value * 1.5));
    return Math.min(1, Math.max(0, 10 ** ((a * (1 - f) + b * f) / 20) / 1.5));
} return lower.value * (1 - f) + upper.value * f; }
/** Prepared per redraw, then O(log n) sample lookup for dense long-duration curves. */
export function preparedAutomation(points: Point[], baseline: number, decibels = false) { const sorted = [...points].sort((a, b) => a.frame - b.frame); return (frame: number) => { let low = 0, high = sorted.length; while (low < high) {
    const middle = (low + high) >>> 1;
    if (sorted[middle]!.frame <= frame)
        low = middle + 1;
    else
        high = middle;
} const lower = sorted[low - 1], upper = sorted[low]; if (!lower)
    return baseline; return automationValue(upper ? [lower, upper] : [lower], frame, baseline, decibels); }; }
export function insertPoint(points: Point[], point: Point) { return [...points.filter(p => p.frame !== point.frame), { ...point, frame: Math.max(0, Math.round(point.frame)), value: Math.min(1, Math.max(0, point.value)) }].sort((a, b) => a.frame - b.frame); }
/** Preserve the previous Read curve before a write, overwrite its interior knots,
 * and retain future original points. A backward seek/loop starts a new span. */
export function writeAutomationPoint(points: Point[], point: Point, previousFrame: number | undefined, baseline: number, decibels = false) {
    const frame = point.frame;
    let next = points;
    if (previousFrame === undefined || frame < previousFrame) {
        if (frame > 0)
            next = insertPoint(next, { frame: frame - 1, value: automationValue(points, frame - 1, baseline, decibels), curve: 'linear', curveStart: 0, curveEnd: 1 });
    }
    else
        next = next.filter(p => p.frame <= previousFrame || p.frame > frame);
    return insertPoint(next, point);
}
export function liveWriteTarget(target:Target):RemoteAutomationWriteTarget {return target.effectID?target.parameter==='amount'?{kind:'effectAmount',effectID:target.effectID}:{kind:'effectParameter',effectID:target.effectID,parameterID:target.parameter}:{kind:target.parameter==='pan'?'pan':'volume'};}
export function targetPoints(track: Track, target: Target,metadata:EffectMetadata[]=[]): Point[] { if (target.effectID){const source=(track.realtimeEffects as Insert[]).find(e=>e.id===target.effectID),meta=metadata.find(m=>m.id===source?.kind),effect=source&&meta?editableEffect(source as RealtimeEffect,meta):source;return (target.parameter==='amount'?source?.automation??[]:effect?.parameterAutomation?.[target.parameter]??[]) as Point[];} return (track.mixAutomation[target.parameter] ?? []) as Point[]; }
export function targetBaseline(track: Track, target: Target, metadata: EffectMetadata[]) { if (target.effectID) {
    const source = (track.realtimeEffects as Insert[]).find(e => e.id === target.effectID),meta=metadata.find(m=>m.id===source?.kind),effect=source&&meta?editableEffect(source as RealtimeEffect,meta):source, parameter=meta?.parameters.find(p=>p.id===target.parameter);
    if(target.parameter==='amount')return source?.amount??.5;
    return effect && parameter ? parameterNormalized(effect.parameters?.[target.parameter] ?? parameter.defaultValue, parameter) : 0.5;
} return target.parameter === 'volume' ? Math.min(1, track.gain / 1.5) : (track.pan + 1) / 2; }
export function controlsWithPoints(track: Track, target: Target, points: Point[],metadata:EffectMetadata[]=[]) { const controls = trackControls(track); if (target.effectID)
    controls.realtimeEffects = (track.realtimeEffects as Insert[]).map(e => {if(e.id!==target.effectID)return e;if(target.parameter==='amount')return {...e,automation:points};const meta=metadata.find(m=>m.id===e.kind),effect=meta?editableEffect(e as RealtimeEffect,meta):e;return {...effect,parameterAutomation:{...effect.parameterAutomation,[target.parameter]:points}};});
else
    controls.mixAutomation = { ...track.mixAutomation, [target.parameter]: points }; return controls; }
export function sameAutomationPoints(a: Point[],b: Point[]) { return a.length===b.length&&a.every((p,i)=>{const q=b[i];return q&&p.frame===q.frame&&Math.fround(p.value)===Math.fround(q.value)&&(p.curve??'linear')===(q.curve??'linear')&&Math.fround(p.curveStart??0)===Math.fround(q.curveStart??0)&&Math.fround(p.curveEnd??1)===Math.fround(q.curveEnd??1);}); }
/** Single acknowledged request plus latest pending value; pointer events never create an unbounded queue. */
export class GestureWriter<T> {
    private pending?: T;
    private active = false;
    private stopped = false;
    private waiters:((success:boolean)=>void)[]=[];
    constructor(private apply: (value: T) => Promise<boolean>) { }
    offer(value: T) { if (this.stopped)
        return; this.pending = value; if (!this.active)
        void this.drain(); }
    private async drain() { this.active=true;try{while(this.pending!==undefined&&!this.stopped){const value=this.pending;this.pending=undefined;let accepted=false;try{accepted=await this.apply(value);}catch{}if(!accepted){this.stopped=true;this.pending=undefined;}}}finally{this.active=false;for(const resolve of this.waiters.splice(0))resolve(!this.stopped);}}
    flush():Promise<boolean>{if(!this.active)return Promise.resolve(!this.stopped);return new Promise(resolve=>this.waiters.push(resolve));}
    cancel() { this.stopped = true; this.pending = undefined; }
    get busy() { return this.active; }
}
export interface AutomationView {
    offset: number;
    pixelsPerSecond: number;
    timebase: 'time' | 'beats';
    snap: boolean;
    follow?: boolean;
}
export class AutomationEditor {
    private canvas: HTMLCanvasElement;
    private ctx: CanvasRenderingContext2D;
    private select: HTMLSelectElement;
    private curve: HTMLSelectElement;
    private readWrite: HTMLSelectElement;
    private valueInput: HTMLInputElement;
    private positionInput: HTMLInputElement;
    private target?: Target;
    private targets: Target[] = [];
    private draft?: Point[];
    private selectedFrame?: number;
    private drag?: {
        start: Point;
        gesture: GestureWriter<Point[]>;
    };
    private width = 0;
    private height = 170;
    private resize: ResizeObserver;
    private frame = 0;
    private dirty = true;
    private canEdit = false;
    private blocked = false;
    private configuredTrack = '';
    private generation = 0;
    private metadata: EffectMetadata[] = [];
    private liveSlider:HTMLInputElement;
    private liveReadout:HTMLOutputElement;
    private liveCancel:HTMLButtonElement;
    private reviewButton:HTMLButtonElement;
    private liveBinding?:TouchControlBinding;
    constructor(private container: HTMLElement, private track: () => Track | undefined, private timing: () => Timing, private view: () => AutomationView, private sampleRate: () => number, private send: (command: Command, trackID: string, expectedTrackRevision?: number) => Promise<unknown>, private report: (message: string) => void, private revision?: (trackID: string) => number | undefined, private navigate?:()=>void, private epoch?:()=>number, private touch?:AutomationTouchBindings,private openReview?:()=>void) {
        const heading = document.createElement('div');
        heading.className = 'automation-toolbar';
        const title = document.createElement('strong');
        title.textContent = 'Automation';
        this.select = document.createElement('select');
        this.select.setAttribute('aria-label', 'Automation lane parameter');
        this.select.onchange = () => { this.reviewServerLane(); this.target = this.targets.find(t => t.id === this.select.value); this.configureValueField();this.liveRefresh(); this.dirty = true; };
        this.curve = document.createElement('select');
        this.curve.setAttribute('aria-label', 'Automation curve');
        for (const c of ['linear', 'smooth', 'easeIn', 'easeOut', 'hold'])
            this.curve.add(new Option(c.replace(/([A-Z])/g, ' $1'), c));
        this.curve.onchange = () => this.changeCurve();
        this.readWrite = document.createElement('select');
        this.readWrite.setAttribute('aria-label', 'Automation Read or Write');
        this.readWrite.add(new Option('Read automation', 'read'));
        const write = new Option('Touch', 'touch');
        write.disabled = true;
        this.readWrite.add(write);
        this.readWrite.title = 'This server doesn’t support live automation recording yet. Edit points and curves directly below.';
        this.readWrite.onchange=()=>{const track=this.track();if(track&&this.target){if(!this.touch?.arm(track.id,liveWriteTarget(this.target),this.readWrite.value==='touch'))this.readWrite.value='read';if(this.readWrite.value==='read')this.liveBinding?.blur();this.liveRefresh();}};
        this.liveSlider=document.createElement('input');this.liveSlider.type='range';this.liveSlider.min='0';this.liveSlider.max='1';this.liveSlider.step='.001';this.liveSlider.setAttribute('aria-label','Touch automation value');
        this.liveReadout=document.createElement('output');this.liveCancel=document.createElement('button');this.liveCancel.textContent='Cancel Touch';this.liveCancel.title='Return to Read and discard this gesture.';this.liveCancel.onclick=()=>void this.touch?.controller.end(true);this.reviewButton=document.createElement('button');this.reviewButton.textContent='Review recordings';this.reviewButton.onclick=()=>this.openReview?.();
        const liveControl=()=>{const track=this.track();if(track&&this.target&&this.touch&&(!this.liveBinding||!this.liveBinding.matches(track.id,liveWriteTarget(this.target))))this.liveBinding=new TouchControlBinding(this.touch,track.id,liveWriteTarget(this.target));return this.liveBinding;};
        this.liveSlider.onpointerdown=()=>{liveControl()?.pointerDown();};this.liveSlider.onkeydown=e=>{if(!e.repeat)liveControl();this.liveBinding?.keyDown(e.key,e.repeat);};this.liveSlider.onkeyup=()=>this.liveBinding?.keyUp(Number(this.liveSlider.value));
        this.liveSlider.oninput=()=>{liveControl();this.liveBinding?.input(Number(this.liveSlider.value));this.liveReadout.textContent=this.physicalValue(Number(this.liveSlider.value)).toPrecision(4);};
        this.liveSlider.onpointerup=()=>this.liveBinding?.pointerUp(Number(this.liveSlider.value));this.liveSlider.onchange=()=>{liveControl();this.liveBinding?.change(Number(this.liveSlider.value));};this.liveSlider.onpointercancel=()=>this.liveBinding?.cancel();this.liveSlider.onblur=()=>this.liveBinding?.blur();
        this.positionInput = document.createElement('input');
        this.positionInput.placeholder = 'Point position';
        this.positionInput.setAttribute('aria-label', 'Automation point position');
        this.positionInput.onchange = () => this.changePosition();
        this.valueInput = document.createElement('input');
        this.valueInput.type = 'number';
        this.valueInput.min = '0';
        this.valueInput.max = '1';
        this.valueInput.step = '0.001';
        this.valueInput.setAttribute('aria-label', 'Automation point normalized value');
        this.valueInput.onchange = () => this.changeValue();
        const remove = document.createElement('button');
        remove.textContent = 'Delete point';
        remove.onclick = () => this.removePoint();
        const smooth = document.createElement('button');
        smooth.textContent = 'Smooth';
        smooth.title = 'Set smooth interpolation across this lane.';
        smooth.onclick = () => this.smooth();
        const close = document.createElement('button');
        close.textContent = '×';
        close.setAttribute('aria-label', 'Close automation');
        close.onclick = () => this.container.hidden = true;
        const review = document.createElement('button'); review.textContent = 'Reload lane'; review.title = 'Discard the private draft and review the current server curve.'; review.onclick = () => this.reviewServerLane();
        heading.append(title, this.select, this.readWrite,this.liveSlider,this.liveReadout,this.liveCancel,this.reviewButton, this.curve, this.positionInput, this.valueInput, remove, smooth, review, close);
        container.append(heading);
        this.canvas = document.createElement('canvas');
        this.canvas.tabIndex = 0;
        this.canvas.setAttribute('role', 'application');
        this.canvas.setAttribute('aria-label', 'Automation chart. Home or End selects the first or last point; brackets select previous or next. Click to add a point, drag to adjust, Delete removes the selected point. Values can also be edited numerically.');
        container.append(this.canvas);
        const ctx = this.canvas.getContext('2d');
        if (!ctx)
            throw new Error('Canvas unavailable.');
        this.ctx = ctx;
        this.resize = new ResizeObserver(() => { this.width = container.clientWidth; const dpr = Math.min(devicePixelRatio, 2); this.canvas.width = this.width * dpr; this.canvas.height = this.height * dpr; this.canvas.style.width = `${this.width}px`; this.canvas.style.height = `${this.height}px`; ctx.setTransform(dpr, 0, 0, dpr, 0, 0); this.dirty = true; });
        this.resize.observe(container);
        this.canvas.onpointerdown = e => this.down(e);
        this.canvas.onpointermove = e => this.move(e);
        this.canvas.onpointerup = e => { if (this.drag) {
            this.drag.gesture.offer(this.points());
            this.drag = undefined;
        } if (this.canvas.hasPointerCapture(e.pointerId))
            this.canvas.releasePointerCapture(e.pointerId); this.dirty = true; };
        this.canvas.onpointercancel = () => { this.drag?.gesture.cancel(); this.drag = undefined; this.draft = undefined; this.dirty = true; };
        this.canvas.onkeydown = e => this.key(e);
        this.frame = requestAnimationFrame(() => this.tick());
    }
    configure(metadata: EffectMetadata[], canEdit: boolean) { const identity=this.track()?.id??'';if(identity!==this.configuredTrack){this.reviewServerLane();this.configuredTrack=identity;}this.metadata = metadata; this.canEdit = canEdit; const track = this.track(), previous = this.target?.id; this.targets = [{ id: 'volume', name: 'Volume', parameter: 'volume' }, { id: 'pan', name: 'Pan', parameter: 'pan' }]; for (const e of (track?.realtimeEffects ?? []) as Insert[]) {
        const m = metadata.find(m => m.id === e.kind);
        if(m&&!Object.keys(e.parameters??{}).length)this.targets.push({id:`${e.id}:amount`,name:`${m.name} · Legacy amount`,effectID:e.id,parameter:'amount'});
        if (m)
            for (const p of m.parameters)
                this.targets.push({ id: `${e.id}:${p.id}`, name: `${m.name} · ${p.name}`, effectID: e.id, parameter: p.id });
    } this.select.replaceChildren(...this.targets.map(t => new Option(t.name, t.id))); this.target = this.targets.find(t => t.id === previous) ?? this.targets[0]; this.select.value = this.target?.id ?? ''; this.configureValueField();this.liveRefresh(); this.dirty = true; }
    liveRefresh(){const track=this.track(),target=this.target?liveWriteTarget(this.target):undefined,armed=!!track&&!!target&&!!this.touch?.armed(track.id,target),supported=!!track&&!!target&&!!this.touch?.supported(track.id,target);const option=this.readWrite.options[1];if(option)option.disabled=!supported;this.readWrite.value=armed?'touch':'read';this.readWrite.title=supported?'Touch records changes to this control while the mix plays. The chart edits explicit points.':'This server or parameter does not support live automation recording yet.';this.liveSlider.hidden=this.liveReadout.hidden=!armed;this.liveSlider.disabled=!supported;this.liveSlider.setAttribute('aria-label',`${this.target?.name??'Automation'} Touch value`);if(track&&this.target&&!this.liveBinding?.active)this.liveSlider.value=String(targetBaseline(track,this.target,this.metadata));const current=this.touch?.controller.current;this.liveCancel.hidden=!this.touch?.controller.busy;this.liveCancel.disabled=current?.phase==='finalizing';this.reviewButton.hidden=!this.openReview||!this.touch?.controller.outcomes.length;if(armed&&current?.trackID===track?.id&&target&&sameLiveTarget(current.target,target))this.liveReadout.textContent=current.phase==='writing'?`Recording · ${current.lastAppliedSequence} applied`:current.reason;}
    private key(e:KeyboardEvent) {
        if(!acceptWorkspaceShortcut(e))return;
        if(isItemNavigation(e.key)&&!e.metaKey&&!e.ctrlKey&&!e.altKey){
            e.preventDefault();e.stopPropagation();const points=this.points(),point=navigateTimelineItem(points,points.find(p=>p.frame===this.selectedFrame),e.key,p=>p.frame);
            if(point){this.selectedFrame=point.frame;this.updateFields(point);const x=this.x(point.frame);if(x<20||x>this.width-20){this.view().offset=Math.max(0,point.frame/this.sampleRate()*this.view().pixelsPerSecond-this.width/2);this.view().follow=false;this.navigate?.();}this.dirty=true;this.report(`Selected ${this.target?.name??'automation'} point at ${this.positionInput.value}, value ${this.valueInput.value}.${this.blocked?' Private draft; Reload lane to review the server version.':''}`);}
        }else if(e.key==='Delete'||e.key==='Backspace'){e.preventDefault();e.stopPropagation();this.removePoint();}
    }
    private reviewServerLane() { this.generation++; this.drag?.gesture.cancel();this.drag=undefined;this.draft=undefined;this.blocked=false;this.canvas?.removeAttribute('aria-description');this.selectedFrame=undefined;if(this.positionInput){this.positionInput.value='';this.positionInput.disabled=true;}if(this.valueInput){this.valueInput.value='';this.valueInput.disabled=true;}this.dirty=true; }
    selectParameter(effectID:string,parameter:string) { this.target=this.targets.find(t=>t.effectID===effectID&&t.parameter===parameter); this.select.value=this.target?.id??''; this.reviewServerLane(); this.configureValueField();this.liveRefresh(); this.dirty=true; }
    currentTarget() { return this.target; }
    invalidate() { this.dirty = true; }
    private points() { return this.draft ?? (this.target && this.track() ? targetPoints(this.track()!, this.target,this.metadata) : []); }
    private x(frame: number) { return frame / this.sampleRate() * this.view().pixelsPerSecond - this.view().offset; }
    private y(value: number) { return 15 + (1 - value) * (this.height - 30); }
    private point(e: PointerEvent): Point { let frame = Math.max(0, Math.round((e.offsetX + this.view().offset) / this.view().pixelsPerSecond * this.sampleRate())); if (this.view().snap)
        frame = snapFrame(frame, this.timing(), this.sampleRate()); return { frame, value: Math.min(1, Math.max(0, 1 - (e.offsetY - 15) / (this.height - 30))), curve: this.curve.value as Curve, curveStart: 0, curveEnd: 1 }; }
    private conflict() { this.blocked=true;this.drag?.gesture.cancel();this.drag=undefined;this.dirty=true;this.canvas.setAttribute('aria-description','Private draft retained after a conflicting edit. Reload lane to review the server curve before editing.');this.report('This track changed during the gesture. Your private curve is retained; Reload lane to review the server version.'); }
    private writer(name: string, target: Target) {
        const generation=this.generation,id=randomUUID().toUpperCase(),trackID=this.track()?.id??'',guard=new TrackGestureRevision(trackID,()=>this.track()?.id,this.revision,this.epoch);
        return new GestureWriter<Point[]>(async points=>{
            if(generation!==this.generation)return false;
            if(!this.drag)this.draft=structuredClone(points);
            if(!this.editingAllowed()){this.report('Finish recording this control before editing its curve.');return false;}
            if(this.blocked||!guard.ready()){this.conflict();return false;}
            const track=this.track();if(!track)return false;
            const result=await this.send({type:'updateTrackControls',controls:controlsWithPoints(track,target,points,this.metadata),name,gestureID:id},trackID,guard.revision);
            if(generation!==this.generation)return false;
            if(!result){this.canvas.setAttribute('aria-description','Private draft retained after a rejected or uncertain command. Review server status before reloading this lane.');this.blocked=true;this.drag=undefined;this.dirty=true;return false;}
            const current=this.track(),unchanged=!!current&&sameAutomationPoints(targetPoints(current,target,this.metadata),points);
            if(!guard.accepted(result,unchanged)){this.conflict();return false;}
            if(!this.drag&&sameAutomationPoints(this.draft??[],points))this.draft=undefined;
            this.dirty=true;return true;
        });
    }
    private editingAllowed(){const current=this.touch?.controller.current,track=this.track();return this.canEdit&&!(this.touch?.controller.busy&&!!current&&current.trackID===track?.id&&this.target&&sameLiveTarget(current.target,liveWriteTarget(this.target)));}
    private down(e: PointerEvent) { if (!this.editingAllowed() || !this.target || this.blocked)
        return; const point = this.point(e), nearest = this.points().find(p => Math.hypot(this.x(p.frame) - e.offsetX, this.y(p.value) - e.offsetY) < 10); this.canvas.focus(); this.canvas.setPointerCapture(e.pointerId); this.draft = structuredClone(this.points()); const active = nearest ?? point; if (!nearest)
        this.draft = insertPoint(this.draft, point); this.selectedFrame = active.frame; this.drag = { start: active, gesture: this.writer('Edit automation', this.target) }; this.curve.value = active.curve; this.updateFields(active); this.dirty = true; }
    private move(e: PointerEvent) { if (!this.drag || !this.draft)
        return; const p = this.point(e), old = this.selectedFrame; this.draft = insertPoint(this.draft.filter(p => p.frame !== old), { ...this.drag.start, frame: p.frame, value: p.value }); this.selectedFrame = p.frame; this.updateFields(p); this.drag.gesture.offer(this.draft); this.dirty = true; }
    private updateFields(p: Point) { this.positionInput.disabled=this.valueInput.disabled=!this.editingAllowed()||this.blocked; this.positionInput.value = formatPosition(p.frame, this.timing(), this.sampleRate(), this.view().timebase); this.valueInput.value = Number(this.physicalValue(p.value).toPrecision(6)).toString(); }
    private parameter() { const t = this.target, e = (this.track()?.realtimeEffects as Insert[] | undefined)?.find(e => e.id === t?.effectID); return this.metadata.find(m => m.id === e?.kind)?.parameters.find(p => p.id === t?.parameter); }
    private physicalValue(value: number) {if(this.target?.parameter==='amount'&&this.target.effectID)return value*100; const p = this.parameter(); if (p)
        return parameterValue(value, p)*inputScale(p); return this.target?.parameter === 'pan' ? (value * 2 - 1) * 100 : value * 1.5; }
    private normalizedValue(value: number) {if(this.target?.parameter==='amount'&&this.target.effectID)return value/100; const p = this.parameter(); if (p)
        return parameterNormalized(value/inputScale(p), p); return this.target?.parameter === 'pan' ? (value / 100 + 1) / 2 : value / 1.5; }
    private configureValueField() { this.positionInput.disabled=this.valueInput.disabled=this.selectedFrame===undefined||!this.editingAllowed()||this.blocked; const p = this.parameter(); this.valueInput.min = String(p ? p.minimum*inputScale(p) : ( (this.target?.parameter === 'pan' ? -100 : 0))); this.valueInput.max = String(p ? p.maximum*inputScale(p) : ( (this.target?.parameter === 'pan' ? 100 : this.target?.parameter==='amount'?100:1.5))); this.valueInput.step = p?.integer ? '1' : String(p ? Math.max(0.0001, (p.maximum - p.minimum)*inputScale(p) / 1000) : 0.001); this.valueInput.setAttribute('aria-label', `${this.target?.name ?? 'Automation'} value ${p?.unit ?? (this.target?.parameter === 'pan' ? 'percent left or right' : this.target?.parameter==='amount'?'percent':'linear gain')}`); }
    private changePosition() { if (!this.target || this.selectedFrame === undefined || !this.editingAllowed() || this.blocked)
        return; try {
        const frame = parsePosition(this.positionInput.value, this.timing(), this.sampleRate(), this.view().timebase), points = this.points(), point = points.find(p => p.frame === this.selectedFrame);
        if (point) {
            this.writer('Move automation point', this.target).offer(insertPoint(points.filter(p => p.frame !== this.selectedFrame), { ...point, frame }));
            this.selectedFrame = frame;
        }
    }
    catch (error) {
        this.report(error instanceof Error ? error.message : 'Invalid position.');
    } }
    private changeCurve() { if (!this.target || this.selectedFrame === undefined || !this.editingAllowed() || this.blocked)
        return; const points = this.points().map(p => p.frame === this.selectedFrame ? { ...p, curve: this.curve.value as Curve, curveStart: 0, curveEnd: 1 } : p); this.writer('Automation curve', this.target).offer(points); }
    private changeValue() { if (!this.target || this.selectedFrame === undefined || !this.editingAllowed() || this.blocked)
        return; const physical = Number(this.valueInput.value); if (!Number.isFinite(physical) || physical < Number(this.valueInput.min) || physical > Number(this.valueInput.max)) {
        this.report('Automation value is outside the parameter range.');
        return;
    } const value = this.normalizedValue(physical); this.writer('Automation value', this.target).offer(this.points().map(p => p.frame === this.selectedFrame ? { ...p, value } : p)); }
    private removePoint() { if (!this.target || this.selectedFrame === undefined || !this.editingAllowed() || this.blocked)
        return; this.writer('Delete automation point', this.target).offer(this.points().filter(p => p.frame !== this.selectedFrame)); this.selectedFrame = undefined; }
    private smooth() { if (!this.target || !this.editingAllowed() || this.blocked)
        return; this.writer('Smooth automation', this.target).offer(this.points().map(p => ({ ...p, curve: 'smooth', curveStart: 0, curveEnd: 1 }))); }
    private tick() { if (this.dirty && !this.container.hidden) {
        this.draw();
        this.dirty = false;
    } this.frame = requestAnimationFrame(() => this.tick()); }
    private draw() { const c = this.ctx, w = this.width, h = this.height, track = this.track(), target = this.target; c.fillStyle = '#141b23'; c.fillRect(0, 0, w, h); if (!track || !target)
        return; const points = this.points(), baseline = targetBaseline(track, target, this.metadata), decibels = target.parameter === 'volume' && (track as unknown as {
        volumeAutomationScale?: string;
    }).volumeAutomationScale === 'decibels'; c.strokeStyle = '#374553'; c.lineWidth = 1; c.font = '10px system-ui'; for (let value = 0; value <= 1; value += 0.25) {
        const y = this.y(value);
        c.beginPath();
        c.moveTo(0, y);
        c.lineTo(w, y);
        c.stroke();
        c.fillStyle = '#9aaebf';
        c.fillText(Number(this.physicalValue(value).toPrecision(4)).toString(), 7, y - 4);
    } const sample = preparedAutomation(points, baseline, decibels); c.strokeStyle = this.blocked?'#edc774':'#8bc2e4'; c.lineWidth = 2; c.beginPath(); for (let x = 0; x < w; x += 2) {
        const frame = (x + this.view().offset) / this.view().pixelsPerSecond * this.sampleRate(), value = sample(frame), y = this.y(value);
        if (x === 0)
            c.moveTo(x, y);
        else
            c.lineTo(x, y);
    } c.stroke(); for (const p of points) {
        const x = this.x(p.frame);
        if (x < -10 || x > w + 10)
            continue;
        c.beginPath();
        c.arc(x, this.y(p.value), p.frame === this.selectedFrame ? 5 : 3.5, 0, Math.PI * 2);
        c.fillStyle = p.frame === this.selectedFrame ? '#f2e6b7' : '#a6d0ec';
        c.fill();
    } }
    destroy() { cancelAnimationFrame(this.frame); this.resize.disconnect(); }
}
