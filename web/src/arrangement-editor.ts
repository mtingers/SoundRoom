import {randomUUID} from './crypto.js';
import { type SessionMirror, type Command, type Track as ViewTrack } from './model.js';
import type { Track, Clip } from './generated/models.js';
import { formatPosition, parsePosition } from './timing.js';
import { copyAudio, copyTrack, pasteTrack, newTrack, trimClip, fragment, deleteRange, resetEnvelope, clipFromHistory, type Clipboard } from './arranging.js';
import { type EffectMetadata } from './effects.js';
import { payload } from './transfers.js';
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, text?: string) => { const e = document.createElement(tag); if (text)
    e.textContent = text; return e; };
type Sender = (command: Command, trackID?: string) => Promise<{
    payload?: unknown;
} | undefined>;
export class ArrangementEditor {
    private clipboard?: Clipboard;
    private signature = '';
    private context?: HTMLDialogElement;
    constructor(private container: HTMLElement, private mirror: SessionMirror, private send: Sender, private report: (message: string) => void, private enabled: () => boolean, private maximum: () => number, private changed: () => void, private metadata: () => EffectMetadata[] = () => []) { }
    private track() { return this.mirror.snapshot?.session.tracks.find(t => t.id === this.mirror.view.selectedTrack) as unknown as Track | undefined; }
    private clip() { return this.track()?.clips?.find(c => this.mirror.view.selectedClips.has(c.id)); }
    private commit(track: Track, name: string, gestureID?: string) { if (!this.enabled())
        return Promise.resolve(undefined); return this.send({ type: 'editTracks', edit: { tracks: [track], removedTrackIDs: [], name, gestureID } }, track.id); }
    private button(label: string, action: () => void, enabled = this.enabled()) { const b = el('button', label); b.disabled = !enabled; b.onclick = action; return b; }
    render() {
        const s = this.mirror.snapshot, v = this.mirror.view, signature = `${s?.revision}:${v.selectedTrack}:${[...v.selectedClips].join(',')}:${v.timebase}:${this.enabled()}`;
        if (this.signature === signature)
            return;
        this.signature = signature;
        this.container.replaceChildren();
        const clip = this.clip();
        if (!clip)
            return;
        const details = el('details');
        details.open = true;
        details.append(el('summary', 'Clip · ' + clip.name));
        const info = el('div');
        info.className = 'clip-properties';
        const timing = s!.session.timing, rate = this.mirror.sampleRate;
        const position = (label: string, value: number, change: (frame: number) => void) => { const l = el('label', label), input = el('input'); input.value = formatPosition(value, timing, rate, v.timebase); input.setAttribute('aria-label', `Clip ${label}`); input.disabled = !this.enabled(); input.onchange = () => { try {
            change(parsePosition(input.value, timing, rate, v.timebase));
        }
        catch (error) {
            this.report(error instanceof Error ? error.message : 'Invalid clip position.');
            this.signature = '';
            this.render();
        } }; l.append(input); info.append(l); };
        position('Start', clip.start, start => void this.changeClip(clip.id, 'Move clip', c => ({ ...c, start })));
        position('End', clip.start + clip.length, end => void this.changeClip(clip.id, 'Trim clip', c => trimClip(c, this.assetFrames(c), undefined, end)));
        position('Source', clip.offset, offset => void this.changeClip(clip.id, 'Slip clip', c => { if (offset + c.length > this.assetFrames(c))
            throw Error('The source offset exceeds the audio file.'); return { ...resetEnvelope(c), offset, soundDesign: undefined, drumPattern: undefined }; }));
        for (const [label, key] of [['Fade in', 'fadeInFrames'], ['Fade out', 'fadeOutFrames']] as const) {
            const l = el('label', `${label} · ms`), input = el('input');
            input.type = 'number';
            input.min = '0';
            input.max = String(clip.length / rate * 1000);
            input.step = '1';
            input.value = String(((clip[key] ?? 0) / rate * 1000).toFixed(1));
            input.disabled = !this.enabled();
            input.onchange = () => { const frames = Math.round(Number(input.value) * rate / 1000); if (!Number.isFinite(frames) || frames < 0 || frames > clip.length) {
                this.report('Fade length must fit inside the clip.');
                return;
            } void this.changeClip(clip.id, label, c => ({ ...resetEnvelope(c), [key]: frames })); };
            l.append(input);
            info.append(l);
        }
        const protection = el('label', 'Protect edges'), toggle = el('input');
        toggle.type = 'checkbox';
        toggle.checked = clip.edgeProtectionDisabled !== true;
        toggle.disabled = !this.enabled();
        toggle.onchange = () => void this.changeClip(clip.id, 'Clip edge protection', c => ({ ...resetEnvelope(c), edgeProtectionDisabled: !toggle.checked, edgeFadeFrames: c.edgeFadeFrames ?? Math.round(rate * 0.005) }));
        protection.append(toggle);
        info.append(protection);
        details.append(info);
        const actions = el('div');
        actions.className = 'clip-actions';
        actions.append(this.button('Trim to range', () => void this.trimSelection(), !!v.range && this.enabled()), this.button('Align to cursor', () => void this.align()), this.button('History', () => this.history(), !!clip.previousStates?.length && this.enabled()));
        details.append(actions);
        const track = this.track()!, sorted = [...(track.clips ?? [])].sort((a, b) => a.start - b.start || a.id.localeCompare(b.id)), next = sorted[sorted.findIndex(c => c.id === clip.id) + 1];
        if (next && this.assetFrames(clip) - clip.offset + clip.start > next.start && next.start > clip.start && next.start <= clip.start + clip.length && next.start + next.length >= clip.start + clip.length) {
            const l = el('label', 'Crossfade · ms'), input = el('input');
            input.type = 'number';
            input.min = String(1000 / rate);
            input.max = String(Math.min(next.length, this.assetFrames(clip) - clip.offset + clip.start - next.start) / rate * 1000);
            input.step = '1';
            input.value = String((Math.max(1, clip.start + clip.length - next.start) / rate * 1000).toFixed(1));
            input.disabled = !this.enabled();
            input.onchange = () => { const frames = Math.round(Number(input.value) * rate / 1000); if (!Number.isFinite(frames) || frames < 1) {
                this.report('Crossfade must contain at least one sample.');
                return;
            } const updated = structuredClone(track), out = updated.clips!.find(c => c.id === clip.id)!, incoming = updated.clips!.find(c => c.id === next.id)!; const end = Math.min(next.start + next.length, out.start + this.assetFrames(out) - out.offset, next.start + frames); Object.assign(out, resetEnvelope(out), { length: end - out.start, edgeProtectionDisabled: false, edgeFadeFrames: Math.max(1, out.edgeFadeFrames ?? Math.round(rate * 0.005)), soundDesign: undefined, drumPattern: undefined }); delete out.renderEnvelope; delete out.fadeReferenceOffset; delete out.fadeReferenceLength; Object.assign(incoming, resetEnvelope(incoming), { edgeProtectionDisabled: false, edgeFadeFrames: Math.max(1, incoming.edgeFadeFrames ?? Math.round(rate * 0.005)) }); delete incoming.renderEnvelope; delete incoming.fadeReferenceOffset; delete incoming.fadeReferenceLength; void this.commit(updated, 'Resize crossfade'); };
            l.append(input);
            details.append(l);
        }
        this.container.append(details);
    }
    private assetFrames(clip: Clip) { const value = this.mirror.snapshot?.assets.find(a => a.id === clip.assetID)?.frames; if (value === undefined)
        throw Error('Refresh the project before editing this source.'); return value; }
    async changeClip(id: string, name: string, change: (clip: Clip) => Clip) { try {
        const track = structuredClone(this.track());
        if (!track)
            return;
        const clip = track.clips?.find(c => c.id === id);
        if (!clip)
            return;
        const changed = change(clip);
        if (changed.start + changed.length > this.maximum())
            throw Error('This edit exceeds the timeline limit.');
        track.clips = track.clips!.map(c => c.id === id ? changed : c);
        if (await this.commit(track, name)) {
            this.mirror.view.range = undefined;
            this.changed();
        }
    }
    catch (error) {
        this.report(error instanceof Error ? error.message : 'Clip edit failed.');
    } }
    async trimBoundary(trackID: string, clipID: string, frame: number, side: 'start' | 'end') { this.mirror.view.selectedTrack = trackID; await this.changeClip(clipID, 'Trim clip', c => trimClip(c, this.assetFrames(c), side === 'start' ? frame : undefined, side === 'end' ? frame : undefined)); }
    copy(cut = false) { const track = this.track(), s = this.mirror.snapshot; if (!track || !s || !this.enabled())
        return; const clips = copyAudio(track, this.mirror.view.selectedClips, this.mirror.view.range); if (!clips.length) {
        this.report('Select audio to copy.');
        return;
    } this.clipboard = { kind: 'audio', serverID: s.serverID, clips }; this.report('Audio copied.'); if (cut)
        void this.cut(track); }
    private async cut(track: Track) { const range = this.mirror.view.range; if (range && this.mirror.view.selectedClips.size <= 1)
        await this.commit(deleteRange(track, range[0], range[1]), 'Cut audio');
    else
        await this.send({ type: 'deleteClips', trackID: track.id, clipIDs: [...this.mirror.view.selectedClips] }, track.id); }
    copyWholeTrack(automation = false) { const track = this.track(), s = this.mirror.snapshot; if (!track || !s || !this.enabled())
        return; this.clipboard = { kind: 'track', serverID: s.serverID, track: copyTrack(track, automation), automation }; this.report(automation ? 'Track copied with automation.' : 'Track copied.'); }
    async paste() {
        const copy = this.clipboard, s = this.mirror.snapshot;
        if (!copy || !s || !this.enabled()) {
            this.report('Copy audio or a track first.');
            return;
        }
        if (copy.serverID !== s.serverID) {
            this.report('This clipboard belongs to another hardware server. Import its audio files first.');
            return;
        }
        try {
            const destination = this.mirror.view.range?.[0] ?? this.mirror.view.cursor;
            let track: Track;
            if (copy.kind === 'track') {
                track = pasteTrack(copy.track, destination, new Set(s.session.tracks.map(t => t.name)), this.maximum(), this.metadata());
                const order = s.session.tracks.map(t => t.id), index = order.indexOf(this.mirror.view.selectedTrack);
                order.splice(index >= 0 ? index + 1 : order.length, 0, track.id!);
                if (!await this.send({ type: 'editTracks', edit: { tracks: [track], removedTrackIDs: [], trackOrder: order, name: copy.automation ? 'Paste track + automation' : 'Paste track' } }))
                    return;
            }
            else {
                track = structuredClone(this.track() ?? newTrack('Audio', s.session.tracks.length % 6));
                const clips = copy.clips.map(c => ({ ...structuredClone(c), id: randomUUID().toUpperCase(), start: c.start + destination }));
                if (clips.some(c => c.start + c.length > this.maximum()))
                    throw Error('The pasted audio exceeds the timeline limit.');
                track.clips = [...(track.clips ?? []), ...clips];
                if (!await this.commit(track, 'Paste audio'))
                    return;
                this.mirror.view.selectedClips = new Set(clips.map(c => c.id));
            }
            this.mirror.view.selectedTrack = track.id!;
            if (copy.kind === 'track')
                this.mirror.view.selectedClips = new Set(track.clips?.map(c => c.id));
            this.mirror.view.range = undefined;
            this.changed();
        }
        catch (error) {
            this.report(error instanceof Error ? error.message : 'Paste failed.');
        }
    }
    async trimSelection() { const track = this.track(), range = this.mirror.view.range; if (!track || !range || range[1] <= range[0])
        return; const changed = structuredClone(track); changed.clips = (track.clips ?? []).map(c => fragment(c, range[0], range[1])).filter((c): c is Clip => !!c); if (await this.commit(changed, 'Trim to selection')) {
        this.mirror.view.selectedClips = new Set(changed.clips.map(c => c.id));
        this.changed();
    } }
    async align() { const clip = this.clip(); if (clip)
        await this.moveSelected(this.mirror.view.cursor - clip.start); }
    async nudge(frames: number) { await this.moveSelected(frames); }
    private async moveSelected(shift: number) { const track = this.track(); if (!track)
        return; const clips = (track.clips ?? []).filter(c => this.mirror.view.selectedClips.has(c.id)); if (!clips.length)
        return; shift = Math.max(shift, -Math.min(...clips.map(c => c.start))); if (clips.some(c => c.start + shift + c.length > this.maximum())) {
        this.report('The moved audio exceeds the timeline limit.');
        return;
    } const changed = structuredClone(track); changed.clips = changed.clips!.map(c => this.mirror.view.selectedClips.has(c.id) ? { ...c, start: c.start + shift } : c); await this.commit(changed, 'Move selected clips'); }
    history() { const clip = this.clip(), track = this.track(); if (!clip || !track)
        return; const dialog = el('dialog'); dialog.className = 'history-dialog'; dialog.append(el('h2', `History · ${clip.name}`)); for (const state of [...(clip.previousStates ?? [])].reverse()) {
        const row = el('div');
        row.className = 'history-state';
        const description = el('div');
        description.append(el('strong', `Before ${state.operation}`), el('span', `${state.name} · ${(state.length / this.mirror.sampleRate).toFixed(2)} s`));
        const restore = (newTrackState: boolean) => { const current = this.track(), currentClip = current?.clips?.find(c => c.id === clip.id); if (!current || !currentClip)
            return; const restored = clipFromHistory(currentClip, state, newTrackState); let target: Track; if (newTrackState) {
            target = newTrack(`${state.name} · ${state.operation}`, this.mirror.snapshot!.session.tracks.length % 6);
            target.clips = [restored];
        }
        else {
            target = structuredClone(current);
            target.clips = target.clips!.map(c => c.id === clip.id ? restored : c);
        } void this.commit(target, newTrackState ? 'Create track from clip history' : 'Restore clip state').then(reply => { if (reply) {
            this.mirror.view.selectedTrack = target.id!;
            this.mirror.view.selectedClips = new Set([restored.id]);
            this.mirror.view.range = undefined;
            dialog.close();
            dialog.remove();
            this.changed();
        } }); };
        row.append(description, this.button('Restore', () => restore(false)), this.button('New track', () => restore(true)));
        dialog.append(row);
    } dialog.append(this.button('Close', () => { dialog.close(); dialog.remove(); }, true)); document.body.append(dialog); dialog.showModal(); dialog.oncancel = () => dialog.remove(); }
    contextMenu(trackID: string, clipID: string) { this.mirror.view.selectedTrack = trackID; this.mirror.view.selectedClips = new Set([clipID]); this.changed(); this.context?.remove(); const dialog = el('dialog'); dialog.className = 'arrangement-menu'; this.context = dialog; dialog.append(el('h2', this.clip()?.name ?? 'Audio clip')); for (const [label, action] of [['Copy audio', () => this.copy()], ['Cut audio', () => this.copy(true)], ['Paste', () => void this.paste()], ['Align to cursor', () => void this.align()], ['History…', () => this.history()], ['Copy track', () => this.copyWholeTrack()], ['Copy track + automation', () => this.copyWholeTrack(true)]] as [
        string,
        () => void
    ][]) {
        dialog.append(this.button(label, () => { dialog.close(); dialog.remove(); action(); }, this.enabled() && (label !== 'History…' || !!this.clip()?.previousStates?.length)));
    } dialog.append(this.button('Close', () => { dialog.close(); dialog.remove(); }, true)); document.body.append(dialog); dialog.showModal(); dialog.oncancel = () => dialog.remove(); }
}
export class ProjectBrowser {
    private dialog: HTMLDialogElement;
    private list: HTMLElement;
    private message: HTMLElement;
    constructor(private send: Sender, private canEdit: () => boolean, private report: (message: string) => void) { this.dialog = el('dialog'); this.dialog.className = 'project-dialog'; this.list = el('div'); this.message = el('p', 'Loading server projects…'); const heading = el('h2', 'Server projects'), close = el('button', 'Close'); close.onclick = () => this.dialog.close(); const refresh = el('button', 'Refresh'); refresh.onclick = () => void this.refresh(); this.dialog.append(heading, this.message, this.list, refresh, close); document.body.append(this.dialog); }
    async open() { this.dialog.showModal(); await this.refresh(); }
    private async refresh() { this.message.textContent = 'Loading…'; const reply = await this.send({ type: 'listProjects' }); if (!reply) {
        this.message.textContent = 'Unable to load projects.';
        return;
    } try {
        const values = payload<unknown>(reply.payload);
        if (!Array.isArray(values) || !values.every(v => typeof v === 'string') || values.length > 10000)
            throw Error('Invalid project catalog.');
        this.projects(values);
    }
    catch (error) {
        this.message.textContent = error instanceof Error ? error.message : 'Unable to load catalog.';
    } }
    projects(values: string[]) { if (!this.dialog.open)
        return; this.message.textContent = values.length ? 'Saved on the hardware server. Opening replaces the shared session.' : 'No saved server projects.'; this.list.replaceChildren(); for (const id of values) {
        const button = el('button', id.replace(/\.soundroom$/, ''));
        button.disabled = !this.canEdit();
        button.onclick = () => { if (!confirm(`Open “${id}” for every connected client?`))
            return; this.message.textContent = 'Opening…'; button.disabled = true; void this.send({ type: 'openProject', id }).then(reply => { if (reply) {
            this.dialog.close();
            this.report('Project opened.');
        }
        else
            button.disabled = !this.canEdit(); }); };
        this.list.append(button);
    } }
}
