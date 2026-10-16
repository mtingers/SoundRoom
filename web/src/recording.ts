import type { Command, SessionMirror } from './model.js';
export interface RecordingSettings {
    preRollSeconds: number;
    offsetMS: number;
    countInBars: number;
    punch: boolean;
    metronome: boolean;
    inputMonitoring: boolean;
}
export function recordingCommand(settings: RecordingSettings, mirror: SessionMirror): Command { if (![settings.preRollSeconds, settings.offsetMS, settings.countInBars].every(Number.isFinite) || settings.preRollSeconds < 0 || settings.preRollSeconds > 30 || settings.offsetMS < -2000 || settings.offsetMS > 2000 || !Number.isInteger(settings.countInBars) || settings.countInBars < 0 || settings.countInBars > 4)
    throw Error('Recording setup is outside its supported range.'); const range = mirror.view.range; if (settings.punch && (!range || range[1] <= range[0]))
    throw Error('Select a timeline range before punch recording.'); return { type: 'editor', intent: { operation: 'transport.record', context: { trackID: mirror.view.selectedTrack || undefined, cursorFrame: mirror.view.cursor, clipIDs: [], rangeStart: range?.[0], rangeEnd: range?.[1] }, numbers: { preRollSeconds: settings.preRollSeconds, offsetMS: settings.offsetMS }, integers: { countInBars: settings.countInBars }, strings: {}, flags: { punch: settings.punch, metronome: settings.metronome, inputMonitoring: settings.inputMonitoring } } }; }
export class RecordingSetup {
    private settings: RecordingSettings = { preRollSeconds: 0, offsetMS: 0, countInBars: 0, punch: false, metronome: false, inputMonitoring: false };
    private dialog: HTMLDialogElement;
    private fields = new Map<keyof RecordingSettings, HTMLInputElement>();
    private error: HTMLElement;
    private routes: HTMLElement;
    private start: HTMLButtonElement;
    constructor(private mirror: SessionMirror, private send: (c: Command) => Promise<unknown>, private permitted: () => boolean, private status: (s: string) => void) { this.dialog = document.createElement('dialog'); this.dialog.className = 'recording-dialog'; const heading = document.createElement('h1'); heading.textContent = 'Recording'; this.routes = document.createElement('div'); this.routes.className = 'recording-routes'; this.dialog.append(heading, this.routes); const grid = document.createElement('div'); grid.className = 'recording-fields'; for (const [name, key, min, max, step] of [['Count-in (bars)', 'countInBars', 0, 4, 1], ['Pre-roll (seconds)', 'preRollSeconds', 0, 30, 0.1], ['Recording offset (ms)', 'offsetMS', -2000, 2000, 0.1]] as const) {
        const label = document.createElement('label');
        label.textContent = name;
        const field = document.createElement('input');
        field.type = 'number';
        field.min = String(min);
        field.max = String(max);
        field.step = String(step);
        label.append(field);
        grid.append(label);
        this.fields.set(key, field);
    } for (const [name, key] of [['Punch selected range', 'punch'], ['Count-in metronome', 'metronome'], ['Monitor through server output', 'inputMonitoring']] as const) {
        const label = document.createElement('label');
        label.className = 'check-label';
        label.textContent = name;
        const field = document.createElement('input');
        field.type = 'checkbox';
        label.append(field);
        grid.append(label);
        this.fields.set(key, field);
    } this.dialog.append(grid); const note = document.createElement('p'); note.className = 'hardware-note'; note.textContent = 'Armed tracks record their assigned server devices and channels. Monitoring uses server outputs; use headphones with microphones.'; this.error = document.createElement('div'); this.error.className = 'error'; this.error.setAttribute('role', 'alert'); const actions = document.createElement('div'); actions.className = 'dialog-actions'; const cancel = document.createElement('button'); cancel.textContent = 'Cancel'; cancel.onclick = () => this.dialog.close(); const save = document.createElement('button'); save.textContent = 'Save setup'; save.onclick = () => void this.save(false); this.start = document.createElement('button'); this.start.className = 'primary record'; this.start.textContent = 'Start recording'; this.start.onclick = () => void this.save(true); actions.append(cancel, save, this.start); this.dialog.append(note, this.error, actions); document.body.append(this.dialog); }
    open() { if (!this.permitted())
        return; for (const [key, field] of this.fields) {
        if (field.type === 'checkbox')
            field.checked = this.settings[key] as boolean;
        else
            field.value = String(this.settings[key]);
    } this.error.textContent = ''; this.routes.replaceChildren(); const tracks = this.mirror.snapshot?.session.tracks.filter(t => t.armed) ?? []; for (const t of tracks) {
        const row = document.createElement('div');
        const device = this.mirror.snapshot?.devices.find(d => d.uid === t.inputDeviceUID);
        row.textContent = `${t.name} · ${device?.name ?? t.inputDeviceName ?? 'Session input'} · channel ${(t.inputChannel ?? 0) + 1}${t.stereoInput ? '–' + ((t.inputChannel ?? 0) + 2) : ''}`;
        this.routes.append(row);
    } if (!tracks.length)
        this.routes.textContent = 'No tracks armed. Arm one or more tracks before recording.'; this.start.disabled = !tracks.length; this.dialog.showModal(); }
    private read(): RecordingSettings { const value = { ...this.settings }; for (const [key, field] of this.fields)
        (value as unknown as Record<string, unknown>)[key] = field.type === 'checkbox' ? field.checked : Number(field.value); recordingCommand(value, this.mirror); return value; }
    private async save(start: boolean) { try {
        if (!this.permitted())
            throw Error('Recording requires a connected operator.');
        const settings = this.read();
        this.start.disabled = true;
        if (start) {
            if (await this.send(recordingCommand(settings, this.mirror))) {
                this.settings = settings;
                this.dialog.close();
            }
        }
        else {
            if (await this.send({ type: 'editor', intent: { operation: 'recording.settings', context: { cursorFrame: 0, clipIDs: [] }, integers: {}, numbers: {}, strings: {}, flags: { inputMonitoring: settings.inputMonitoring } } })) {
                this.settings = settings;
                this.dialog.close();
                this.status('Recording setup saved for this client. Monitoring changed at the server.');
            }
        }
    }
    catch (error) {
        this.error.textContent = error instanceof Error ? error.message : 'Recording setup failed.';
    }
    finally {
        this.start.disabled = false;
    } }
    async record() { if (!this.permitted())
        return; try {
        if (this.mirror.snapshot?.transport.isRecording)
            await this.send({ type: 'editor', intent: { operation: 'transport.finishRecording', context: { cursorFrame: 0, clipIDs: [] }, integers: {}, numbers: {}, strings: {}, flags: {} } });
        else
            await this.send(recordingCommand(this.settings, this.mirror));
    }
    catch (error) {
        this.status(error instanceof Error ? error.message : 'Recording failed.');
    } }
}
