import { acceptWorkspaceShortcut, isItemNavigation, navigateTimelineItem, openWorkspaceDialog } from './input-focus.js';
import type { SoundDesignController, SoundDesignControllerKind, SoundDesignPattern } from './generated/models.js';
export function controllerBaseline(kind: SoundDesignControllerKind) { return kind === 'expression' ? 1 : kind === 'pitchBend' ? 0.5 : 0; }
export function controllerValue(points: SoundDesignController[], kind: SoundDesignControllerKind, beat: number) { const sorted = points.filter(p => p.kind === kind).sort((a, b) => a.beat - b.beat); let low = 0, high = sorted.length; while (low < high) {
    const middle = (low + high) >>> 1;
    if (sorted[middle]!.beat <= beat)
        low = middle + 1;
    else
        high = middle;
} return low ? sorted[low - 1]!.value : controllerBaseline(kind); }
/** Sorted single-lane query: retain the held value before the viewport without scanning the prefix. */
export function controllerWindow(sorted: readonly SoundDesignController[], start: number, end: number) {
    const bound = (beat: number, inclusive: boolean) => { let low = 0, high = sorted.length; while (low < high) { const middle = (low + high) >>> 1; if (sorted[middle]!.beat < beat || inclusive && sorted[middle]!.beat === beat) low = middle + 1; else high = middle; } return low; };
    const first = bound(start, false), last = bound(end, true);
    return { first, last, previous: first ? sorted[first - 1] : undefined };
}
export function editController(points: SoundDesignController[], original: SoundDesignController | undefined, point: SoundDesignController, beats: number) { if (!Number.isFinite(point.beat) || point.beat < 0 || point.beat > beats || !Number.isFinite(point.value) || point.value < 0 || point.value > 1)
    throw Error('Controller point is outside the pattern.'); const next = points.filter(p => p !== original && !(p.kind === point.kind && Math.abs(p.beat - point.beat) < 1e-9)); if (next.length >= 4096)
    throw Error('A pattern supports 4096 controller points.'); return [...next, point].sort((a, b) => a.beat - b.beat); }
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, text?: string) => { const e = document.createElement(tag); if (text)
    e.textContent = text; return e; };
export class ControllerEditor {
    private dialog: HTMLDialogElement;
    private canvas: HTMLCanvasElement;
    private context: CanvasRenderingContext2D;
    private kinds: HTMLSelectElement;
    private beat: HTMLInputElement;
    private value: HTMLInputElement;
    private status: HTMLElement;
    private width = 800;
    private height = 260;
    private selected?: SoundDesignController;
    private dragging = false;
    private startBeat = 0;
    private length = 4;
    private chartDirty = true;
    private sorted: SoundDesignController[] = [];
    private indexDirty = true;
    private chartResize: ResizeObserver;
    constructor(private pattern: () => SoundDesignPattern | undefined, private before: () => void, private changed: () => void, private enabled: () => boolean, private audition: () => void) { this.dialog = el('dialog'); this.dialog.className = 'controller-dialog'; this.dialog.setAttribute('aria-label','Performance controller editing'); const toolbar = el('div'); toolbar.className = 'workspace-toolbar'; toolbar.append(el('strong', 'Performance controllers')); this.kinds = el('select'); this.kinds.setAttribute('aria-label', 'Controller lane'); for (const kind of ['sustain', 'modulation', 'expression', 'pitchBend'])
        this.kinds.add(new Option(kind, kind)); this.kinds.onchange = () => { this.selected = undefined; this.indexDirty = true; this.chartDirty = true; }; const minus = el('button', '−'), plus = el('button', '＋'), fit = el('button', 'Fit'), done = el('button', 'Done'); minus.setAttribute('aria-label', 'Zoom out controller chart'); plus.setAttribute('aria-label', 'Zoom in controller chart'); minus.onclick = () => this.zoom(2); plus.onclick = () => this.zoom(0.5); fit.onclick = () => { this.startBeat = 0; this.length = this.pattern()?.beats ?? 4; this.chartDirty = true; }; done.onclick = () => this.dialog.close(); toolbar.append(this.kinds, minus, plus, fit, done); this.canvas = el('canvas'); this.canvas.tabIndex = 0; this.canvas.setAttribute('role', 'application'); this.canvas.setAttribute('aria-label', 'Held MIDI controller values. Home or End selects the first or last point; brackets select previous or next. Click to add, drag to move. Delete removes the selected point, arrows adjust beat or value, Space auditions the Designer.'); this.context = this.canvas.getContext('2d')!; const controls = el('div'); controls.className = 'controller-values'; this.beat = el('input'); this.beat.type = 'number'; this.beat.min = '0'; this.beat.step = '0.0625'; this.beat.setAttribute('aria-label', 'Controller beat'); this.value = el('input'); this.value.type = 'number'; this.value.min = '0'; this.value.max = '1'; this.value.step = '0.01'; this.value.setAttribute('aria-label', 'Controller value'); for (const [name, field] of [['Beat', this.beat], ['Value', this.value]] as const) {
        const label = el('label', name);
        label.append(field);
        controls.append(label);
        field.onchange = () => this.changeSelected(Number(this.beat.value), Number(this.value.value));
    } const remove = el('button', 'Delete point'); remove.onclick = () => this.remove(); controls.append(remove); this.status = el('div', 'Native held events. MIDI values change at each point; the instrument smooths transitions internally.'); this.status.className = 'workspace-status'; this.dialog.append(toolbar, this.canvas, controls, this.status); document.body.append(this.dialog); this.chartResize = new ResizeObserver(() => { this.width = this.canvas.clientWidth; const dpr = Math.min(devicePixelRatio, 2); this.canvas.width = this.width * dpr; this.canvas.height = this.height * dpr; this.context.setTransform(dpr, 0, 0, dpr, 0, 0); this.chartDirty = true; }); this.chartResize.observe(this.canvas); this.canvas.onpointerdown = e => this.down(e); this.canvas.onpointermove = e => this.move(e); this.canvas.onpointerup = e => { this.dragging = false; if (this.canvas.hasPointerCapture(e.pointerId))
        this.canvas.releasePointerCapture(e.pointerId); }; this.canvas.onpointercancel = () => this.dragging = false; this.canvas.onwheel = e => { e.preventDefault(); if (e.ctrlKey || e.metaKey)
        this.zoom(Math.exp(e.deltaY * 0.003), (e.offsetX - 45) / (this.width - 60));
    else {
        this.startBeat = Math.max(0, Math.min((this.pattern()?.beats ?? 4) - this.length, this.startBeat + (e.deltaX || e.deltaY) * this.length / (this.width - 60)));
        this.chartDirty = true;
    } }; this.dialog.onkeydown = e => { if (!acceptWorkspaceShortcut(e))
        return; if (e.code === 'Space') {
        e.preventDefault();
        this.audition();
    }
    else if(isItemNavigation(e.key) && !e.metaKey && !e.ctrlKey && !e.altKey){e.preventDefault();this.prepareIndex();const point=navigateTimelineItem(this.sorted,this.selected,e.key,p=>p.beat);if(point){this.selected=point;if(point.beat<this.startBeat||point.beat>this.startBeat+this.length)this.startBeat=Math.max(0,Math.min((this.pattern()?.beats??4)-this.length,point.beat-this.length/2));this.fields();this.chartDirty=true;this.status.textContent=`Selected ${this.kinds.value} at beat ${(point.beat+1).toFixed(3)}, value ${point.value.toFixed(3)}. Arrow keys adjust this point.`;}}
    else if (e.key === 'Delete' || e.key === 'Backspace') {
        e.preventDefault();
        this.remove();
    }
    else if (this.selected && e.key.startsWith('Arrow')) {
        e.preventDefault();
        this.changeSelected(Math.max(0, Math.min(this.pattern()?.beats ?? 4, this.selected.beat + (e.key === 'ArrowLeft' ? -0.0625 : e.key === 'ArrowRight' ? 0.0625 : 0))), Math.max(0, Math.min(1, this.selected.value + (e.key === 'ArrowUp' ? 0.01 : e.key === 'ArrowDown' ? -0.01 : 0))));
    } }; requestAnimationFrame(() => this.tick()); }
    invalidate() { this.indexDirty = true; this.chartDirty = true; if (this.selected && !this.pattern()?.controllers?.includes(this.selected))
        this.selected = undefined; this.fields(); }
    open() { this.startBeat = 0; this.length = this.pattern()?.beats ?? 4; this.selected = undefined; this.indexDirty = true; this.chartDirty = true; openWorkspaceDialog(this.dialog,this.canvas); this.fields(); }
    private zoom(factor: number, anchor = 0.5) { const beats = this.pattern()?.beats ?? 4, old = this.length, next = Math.max(0.125, Math.min(beats, old * factor)); this.startBeat = Math.max(0, Math.min(beats - next, this.startBeat + (old - next) * Math.max(0, Math.min(1, anchor)))); this.length = next; this.chartDirty = true; }
    private fields() { this.beat.max = String(this.pattern()?.beats ?? 4); this.beat.value = String(this.selected?.beat ?? 0); this.value.value = String(this.selected?.value ?? controllerBaseline(this.kinds.value as SoundDesignControllerKind)); this.beat.disabled = this.value.disabled = !this.selected || !this.enabled(); }
    private point(e: PointerEvent): SoundDesignController { return { kind: this.kinds.value as SoundDesignControllerKind, beat: Math.max(0, Math.min(this.pattern()?.beats ?? 4, Math.round((this.startBeat + (e.offsetX - 45) / (this.width - 60) * this.length) * 16) / 16)), value: Math.max(0, Math.min(1, 1 - (e.offsetY - 20) / (this.height - 45))) }; }
    private down(e: PointerEvent) { if (e.button !== 0 || !this.enabled())
        return; const pattern = this.pattern(); if (!pattern)
        return; const point = this.point(e); this.prepareIndex();
        const tolerance = 10 * this.length / Math.max(1, this.width - 60), rawBeat = this.startBeat + (e.offsetX - 45) / Math.max(1, this.width - 60) * this.length, window = controllerWindow(this.sorted, rawBeat - tolerance, rawBeat + tolerance);
        let nearest: SoundDesignController | undefined;
        for (let i = window.first; i < window.last; i++) { const p = this.sorted[i]!; if (Math.hypot(45 + (p.beat - this.startBeat) / this.length * (this.width - 60) - e.offsetX, 20 + (1 - p.value) * (this.height - 45) - e.offsetY) < 10) { nearest = p; break; } }
        this.before(); this.selected = nearest; this.dragging = true; this.canvas.setPointerCapture(e.pointerId); this.canvas.focus(); if (nearest) {
        this.fields();
        this.chartDirty = true;
    }
    else
        this.install(point); }
    private move(e: PointerEvent) { if (this.dragging && this.enabled())
        this.install(this.point(e), this.selected); }
    private install(point: SoundDesignController, original?: SoundDesignController) { const pattern = this.pattern(); if (!pattern)
        return; try {
        pattern.controllers = editController(pattern.controllers ?? [], original, point, pattern.beats ?? 4);
        this.selected = point;
        this.indexDirty = true;
        this.chartDirty = true;
        this.fields();
        this.changed();
    }
    catch (error) {
        this.status.textContent = String(error);
    } }
    private changeSelected(beat: number, value: number) { if (!this.selected || !this.enabled())
        return; this.before(); this.install({ ...this.selected, beat, value }, this.selected); }
    private remove() { if (!this.selected || !this.enabled())
        return; this.before(); const pattern = this.pattern(); if (pattern)
        pattern.controllers = pattern.controllers?.filter(p => p !== this.selected); this.selected = undefined; this.indexDirty = true; this.chartDirty = true; this.fields(); this.changed(); }
    private tick() { if (this.dialog.open && this.chartDirty) {
        this.draw();
        this.chartDirty = false;
    } requestAnimationFrame(() => this.tick()); }
    private prepareIndex() { if (this.indexDirty) {
        this.sorted = (this.pattern()?.controllers ?? []).filter(p => p.kind === this.kinds.value).sort((a, b) => a.beat - b.beat);
        this.indexDirty = false;
    } }
    private draw() { const c = this.context, w = this.width, h = this.height; this.prepareIndex();
    const window = controllerWindow(this.sorted, this.startBeat, this.startBeat + this.length);
    c.fillStyle = '#15202b'; c.fillRect(0, 0, w, h); c.font = '11px system-ui'; for (let i = 0; i <= 4; i++) {
        const value = i / 4, y = 20 + (1 - value) * (h - 45);
        c.strokeStyle = '#34495c';
        c.beginPath();
        c.moveTo(45, y);
        c.lineTo(w - 15, y);
        c.stroke();
        c.fillStyle = '#9eb5c9';
        c.fillText(String(value), 4, y + 4);
    } const x = (beat: number) => 45 + (beat - this.startBeat) / this.length * (w - 60), y = (value: number) => 20 + (1 - value) * (h - 45), initial = window.previous?.value ?? controllerBaseline(this.kinds.value as SoundDesignControllerKind);
    c.strokeStyle = '#92bfce'; c.lineWidth = 2; c.beginPath(); c.moveTo(45, y(initial)); let previous = initial;
    for (let i = window.first; i < window.last; i++) { const p = this.sorted[i]!;
        c.lineTo(x(p.beat), y(previous));
        c.lineTo(x(p.beat), y(p.value));
        previous = p.value;
    } c.lineTo(w - 15, y(previous)); c.stroke(); for (let i = window.first; i < window.last; i++) { const p = this.sorted[i]!;
        c.fillStyle = p === this.selected ? '#eee0a6' : '#a8cedd';
        c.beginPath();
        c.arc(x(p.beat), y(p.value), p === this.selected ? 5 : 3, 0, Math.PI * 2);
        c.fill();
    } for (let i = 0; i <= 4; i++) {
        c.fillStyle = '#9eb5c9';
        c.fillText((this.startBeat + this.length * i / 4).toFixed(2), 45 + (w - 60) * i / 4, h - 5);
    } }
}
