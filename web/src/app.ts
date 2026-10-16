import {loadPairing,savePairing,forgetPairing} from './remembered-pairing.js';
import {randomUUID} from './crypto.js';
import {AutomationWriteReview} from './write-review.js';
import {AutomationWriteController,automationWriteAvailable} from './automation-write.js';
import {AutomationTouchBindings,TouchControlBinding} from './automation-touch.js';
import type {RemoteAutomationWriteLeaseUpdate} from './generated/models.js';
import { acceptWorkspaceShortcut, openWorkspaceDialog } from './input-focus.js';
import { EffectLibrary } from './effect-presets.js';
import { RecordingSetup } from './recording.js';
import { revisionBarrier } from './revision-barrier.js';
import { PitchWorkspace } from './pitch-workspace.js';
import { EffectWorkspace } from './effect-workspace.js';
import type { EffectMeterSnapshot } from './generated/models.js';
import { MasterWorkspace } from './master-workspace.js';
import { ArrangementEditor, ProjectBrowser } from './arrangement-editor.js';
import { validateModel } from './validate.js';
import { CreativeWorkspaces } from './workspaces.js';
import { SessionMirror, validateSnapshot, validateTransport, validateDevices, validateAsset, validateDelta, type Command, type Snapshot, type Delta, type Asset, type Transport, type Device } from './model.js';
import { SoundroomClient, validateCredential, CommandError } from './sdk.js';
import { Timeline, TRACK_HEIGHT } from './timeline.js';
import { formatPosition, parsePosition } from './timing.js';
import { WaveformCache, WaveformLoader } from './waveforms.js';
import { AutomationEditor, GestureWriter } from './automation.js';
import { EffectsRack, trackControls, type EffectMetadata } from './effects.js';
import { SnapshotAssembler } from './snapshots.js';
import { SnapshotOverlays } from './snapshot-overlays.js';
import { importAudio, downloadMix, payload, type ExportSink } from './transfers.js';
const $ = <T extends HTMLElement = HTMLElement>(id: string) => { const e = document.getElementById(id); if (!e)
    throw new Error(`Missing element ${id}`); return e as T; };
const client = new SoundroomClient(), mirror = new SessionMirror(), cache = new WaveformCache(), assembler = new SnapshotAssembler();
const pendingDeltas: Delta[] = [];
let pendingDeltaBytes = 0;
const snapshotOverlays = new SnapshotOverlays();
let vertical = 0, transfer: AbortController | undefined, editingControls = false, renderRequested = false, sessionReady = false;
const revisionEvents = new EventTarget();
async function waitRevision(revision: number, serverID: string) { return revisionBarrier(revision, () => mirror.snapshot?.revision ?? -1, () => client.state === 'connected' && mirror.snapshot?.serverID === serverID, revisionEvents); }
const setStatus = (message: string) => { $('status').textContent = message; };
async function send(command: Command, trackID?: string, expectedTrackRevision?:number, expectedSessionRevision?:number) {
    try {
        const revisions: Record<string, number> = {};
        const snapshot = mirror.snapshot;
        if (trackID && snapshot)
            revisions[trackID] = expectedTrackRevision ?? snapshot.trackRevisions[trackID] ?? 0;
        if (command.type === 'editTracks' && snapshot) {
            const edit = command.edit as {
                tracks: {
                    id: string;
                }[];
                removedTrackIDs: string[];
                trackOrder?: string[];
            };
            const existing = new Set([...snapshot.session.tracks.map(t => t.id), snapshot.session.masterTrack.id]);
            for (const id of [...edit.tracks.map(t => t.id), ...edit.removedTrackIDs, ...(edit.trackOrder ?? [])])
                if (existing.has(id))
                    revisions[id] = snapshot.trackRevisions[id] ?? 0;
        }
        const reply = await client.command(command, expectedSessionRevision ?? mirror.snapshot?.revision, revisions);
        if (reply.revision > (mirror.snapshot?.revision ?? 0) && mirror.snapshot && !(await waitRevision(reply.revision, mirror.snapshot.serverID))) {
            sessionReady = false;
            updatePermissions();
            setStatus('Edit accepted; waiting for authoritative metadata. Refreshing before further changes.');
            void client.command({ type: 'snapshot' });
            return undefined;
        }
        if (reply.message)
            setStatus(reply.message);
        return reply;
    }
    catch (error) {
        setStatus(error instanceof Error ? error.message : 'Command failed.');
        if (error instanceof CommandError && error.reply.status === 'conflict')
            void client.command({ type: 'snapshot' });
        return undefined;
    }
}
function editor(operation: string, numbers: Record<string, number> = {}, integers: Record<string, number> = {}) {
    return send({ type: 'editor', intent: { operation, context: { trackID: mirror.view.selectedTrack || undefined, clipID: [...mirror.view.selectedClips][0], clipIDs: [...mirror.view.selectedClips], cursorFrame: mirror.view.cursor, rangeStart: mirror.view.range?.[0], rangeEnd: mirror.view.range?.[1] }, numbers, integers, strings: {}, flags: {} } }, mirror.view.selectedTrack);
}
const creative = new CreativeWorkspaces(send, () => mirror.snapshot?.session.tracks.find(t => t.id === mirror.view.selectedTrack), () => mirror.view.cursor, () => mirror.snapshot?.session.timing ?? { tempo: 120, beatsPerBar: 4, beatUnit: 4 }, () => client.clientID, () => mirror.snapshot?.session.tracks.find(t => t.id === mirror.view.selectedTrack)?.clips.find(c => mirror.view.selectedClips.has(c.id)), {track:id=>mirror.snapshot?.session.tracks.find(t=>t.id===id),version:id=>mirror.snapshot?.trackRevisions[id],sessionRevision:()=>mirror.snapshot?.revision,epoch:()=>client.connectionEpoch});
function configureCreative() { creative.configure(client.capabilities, client.state === 'connected' && sessionReady && client.role !== 'viewer', client.state === 'connected' && sessionReady && client.role === 'operator'); }
const arranging = new ArrangementEditor($('clip-editor'), mirror, send, setStatus, () => client.state === 'connected' && sessionReady && client.role !== 'viewer', () => Number(client.capabilities.maxTimelineFrame ?? mirror.sampleRate * 86400), () => { updateSelection(); timeline.invalidate(); }, () => client.capabilities.effects as EffectMetadata[] ?? []);
const recording = new RecordingSetup(mirror,send,()=>client.state==='connected'&&sessionReady&&client.role==='operator',setStatus);
const projects = new ProjectBrowser(send, () => client.state === 'connected' && sessionReady && client.role !== 'viewer', setStatus);
const pitch = new PitchWorkspace(mirror, send, setStatus, () => client.capabilities.effects as EffectMetadata[] ?? [], () => client.state === 'connected' && sessionReady && client.role !== 'viewer', () => client.state === 'connected' && sessionReady && client.role === 'operator', () => client.capabilities.editorOperations as string[] ?? [], () => client.connectionEpoch);
const master = new MasterWorkspace(mirror, send, () => client.capabilities.effects as EffectMetadata[] ?? [], () => client.state === 'connected' && sessionReady && client.role !== 'viewer', () => client.state === 'connected' && sessionReady && client.role === 'operator', () => { $<HTMLSelectElement>('effects-scope').value = 'master'; rackSignature = ''; renderEffects(); }, () => client.connectionEpoch);
const waveformLoader = new WaveformLoader(async (assetID, start, end, points) => { const reply = await client.command({ type: 'waveform', assetID, start, end, points }), asset = payload<Asset>(reply.payload); if (!validateAsset(asset))
    throw new Error('Invalid waveform.'); return asset; }, asset => { cache.put(asset); timeline.waveformReceived(); }, setStatus);
const timeline = new Timeline($('timeline'), mirror, cache, () => { updateSelection(); syncNavigation(); automation.invalidate(); }, (trackID, clipID, start) => void send({ type: 'moveClip', trackID, clipID, start }, trackID), (assetID, start, end, points) => waveformLoader.request(assetID, start, end, points, start === 0 && end === mirror.snapshot?.assets.find(a => a.id === assetID)?.frames), { canEdit: () => client.state === 'connected' && sessionReady && client.role !== 'viewer', trim: (trackID, clipID, frame, side) => void arranging.trimBoundary(trackID, clipID, frame, side), context: (trackID, clipID) => arranging.contextMenu(trackID, clipID) });
const effectLibrary = new EffectLibrary({ getItem: key => localStorage.getItem(key), setItem: (key, value) => localStorage.setItem(key, value) }, () => client.capabilities.effects as EffectMetadata[] ?? []);
const automationWrite=new AutomationWriteController({available:()=>automationWriteAvailable(client.capabilities,client.role,client.state==='connected'&&sessionReady),epoch:()=>client.connectionEpoch,serverID:()=>mirror.snapshot?.serverID,credentialID:()=>client.credentialIdentity,runtimeID:()=>client.clientID,version:id=>mirror.snapshot?.trackRevisions[id],track:id=>id===mirror.snapshot?.session.masterTrack.id?mirror.snapshot.session.masterTrack:mirror.snapshot?.session.tracks.find(t=>t.id===id),metadata:()=>client.capabilities.effects as EffectMetadata[]??[],playing:()=>!!mirror.snapshot?.transport.isPlaying,looping:()=>mirror.snapshot?.transport.loopStart!==undefined&&mirror.snapshot?.transport.loopEnd!==undefined,send:(command,versions={})=>client.command(command,undefined,versions),changed:()=>{automation.liveRefresh();writeReview.refresh();setStatus(automationWrite.current?.reason??'');}});
const touch=new AutomationTouchBindings(automationWrite,setStatus);
const writeReview=new AutomationWriteReview(automationWrite,command=>client.command(command),()=>mirror.sampleRate,id=>id===mirror.snapshot?.session.masterTrack.id?'Master':mirror.snapshot?.session.tracks.find(t=>t.id===id)?.name??'Track');
const effectsRack = new EffectsRack($('effects-rack'), send, () => $<HTMLSelectElement>('effects-scope').value === 'master' ? mirror.snapshot?.session.masterTrack : mirror.snapshot?.session.tracks.find(t => t.id === mirror.view.selectedTrack), () => mirror.view.range, solo => void send({ type: 'editor', intent: { operation: 'transport.play', context: { trackID: mirror.view.selectedTrack, cursorFrame: mirror.view.cursor, clipIDs: [] }, numbers: {}, integers: {}, strings: solo ? { trackID: mirror.view.selectedTrack } : {}, flags: {} } }), (effectID:string,trackID?:string) => pitch.open(effectID,trackID), (trackID,effectID) => effectWorkspace.open(trackID,effectID), trackID=>{mirror.view.selectedTrack=trackID;$('automation-editor').hidden=false;renderAutomation();updateSelection();}, effectLibrary,touch);
let rackSignature = '';
function renderEffects() { const signature = `${$<HTMLSelectElement>('effects-scope').value}:${mirror.view.selectedTrack}:${mirror.snapshot?.revision}:${sessionReady}:${client.role}`; if (signature === rackSignature)
    return; rackSignature = signature; effectsRack.render((client.capabilities.effects as EffectMetadata[]) ?? [], client.state === 'connected' && sessionReady && client.role !== 'viewer', client.state === 'connected' && sessionReady && client.role === 'operator'); }
const automation = new AutomationEditor($('automation-editor'), () => mirror.view.selectedTrack === mirror.snapshot?.session.masterTrack.id ? mirror.snapshot.session.masterTrack : mirror.snapshot?.session.tracks.find(t => t.id === mirror.view.selectedTrack), () => mirror.snapshot?.session.timing ?? { tempo: 120, beatsPerBar: 4, beatUnit: 4 }, () => mirror.view, () => mirror.sampleRate, send, setStatus, trackID=>mirror.snapshot?.trackRevisions[trackID],()=>{syncNavigation();timeline.invalidate();},()=>client.connectionEpoch,touch,()=>writeReview.open());
const effectWorkspace = new EffectWorkspace(mirror, send, () => client.capabilities.effects as EffectMetadata[] ?? [], () => client.state === 'connected' && sessionReady && client.role !== 'viewer', () => client.state === 'connected' && sessionReady && client.role === 'operator', (effectID:string,trackID?:string) => pitch.open(effectID,trackID), (trackID,effectID,parameter) => { mirror.view.selectedTrack=trackID; $('automation-editor').hidden=false; renderAutomation(); automation.selectParameter(effectID,parameter); updateSelection(); }, effectLibrary, ()=>client.connectionEpoch,touch);
let automationSignature = '';
function renderAutomation() { const signature = `${mirror.view.selectedTrack}:${mirror.snapshot?.revision}:${sessionReady}:${client.role}`; if (signature !== automationSignature) {
    automationSignature = signature;
    automation.configure((client.capabilities.effects as EffectMetadata[]) ?? [], client.state === 'connected' && sessionReady && client.role !== 'viewer');
} automation.invalidate(); }
function updateSelection() {
    const s = mirror.snapshot, v = mirror.view;
    if (!s)
        return;
    const format = (n: number) => formatPosition(n, s.session.timing, mirror.sampleRate, v.timebase);
    $('selected-track').textContent = (v.selectedTrack===s.session.masterTrack.id?s.session.masterTrack:s.session.tracks.find(t => t.id === v.selectedTrack))?.name ?? 'No track';
    for (const [id, frame] of [['cursor', v.cursor], ['range-start', v.range?.[0] ?? v.cursor], ['range-end', v.range?.[1] ?? v.cursor]] as const) {
        const field = $<HTMLInputElement>(id);
        if (document.activeElement !== field)
            field.value = format(frame);
    }
    renderEffects();
    renderAutomation();
    arranging.render();
    $('follow').setAttribute('aria-pressed', String(v.follow));
    $('snap').setAttribute('aria-pressed', String(v.snap));
    for (const row of $('track-controls').children)
        row.classList.toggle('selected', (row as HTMLElement).dataset.id === v.selectedTrack);
}
function syncNavigation() { const s = mirror.snapshot; if (!s)
    return; const end = Math.max(mirror.sampleRate * 60, ...s.session.tracks.flatMap(t => t.clips.map(c => c.start + c.length))); $<HTMLInputElement>('scroll').max = String(Math.ceil(Math.max(0, end / mirror.sampleRate * mirror.view.pixelsPerSecond - $('timeline').clientWidth))); $<HTMLInputElement>('scroll').value = String(mirror.view.offset); }
function scheduleRender() { revisionEvents.dispatchEvent(new Event('revision')); if (renderRequested)
    return; renderRequested = true; requestAnimationFrame(() => { renderRequested = false; renderSession(); }); }
function routeOptions(select: HTMLSelectElement, devices: Device[], direction: 'input' | 'output', selected: string | number | undefined, follow = false) { select.replaceChildren(); if (follow)
    select.add(new Option('Session input', '')); for (const d of devices)
    if ((direction === 'input' ? d.inputChannels : d.outputChannels) > 0)
        select.add(new Option(d.name, typeof selected === 'number' ? String(d.id) : d.uid)); select.value = selected === undefined ? '' : String(selected); }
function renderTrackControls() {
    const s = mirror.snapshot, container = $('track-controls');
    if (!s || editingControls)
        return;
    container.replaceChildren();
    const first = Math.max(0, Math.floor(vertical / TRACK_HEIGHT)), last = Math.min(s.session.tracks.length, Math.ceil((vertical + container.clientHeight) / TRACK_HEIGHT));
    for (let index = first; index < last; index++) {
        const t = s.session.tracks[index]!, row = document.createElement('div');
        row.className = 'track';
        row.classList.toggle('selected', t.id === mirror.view.selectedTrack);
        row.dataset.id = t.id;
        row.style.top = `${index * TRACK_HEIGHT - vertical}px`;
        row.style.setProperty('--color', ['#82b5cf', '#ad9bd8', '#76c4aa', '#d1aa74', '#d0849d', '#96b479'][Math.abs(t.color) % 6]!);
        const title = document.createElement('div');
        title.className = 'track-title';
        const name = document.createElement('button');
        name.className = 'track-name';
        name.textContent = t.name;
        name.title = t.name;
        name.onclick = () => { mirror.view.selectedTrack = t.id; mirror.view.selectedClips.clear(); updateSelection(); timeline.invalidate(); };
        name.ondblclick = () => { const value = prompt('Track name', t.name); if (value?.trim())
            void send({ type: 'setTrackName', trackID: t.id, name: value.trim() }, t.id); };
        title.append(name);
        const buttons = document.createElement('div');
        buttons.className = 'track-buttons';
        for (const [label, key, type] of [['R', 'armed', 'setTrackArm'], ['M', 'muted', 'setTrackMute'], ['S', 'solo', 'setTrackSolo']] as const) {
            const b = document.createElement('button');
            b.textContent = label;
            b.setAttribute('aria-label', `${label === 'R' ? 'Arm' : label === 'M' ? 'Mute' : 'Solo'} ${t.name}`);
            b.setAttribute('aria-pressed', String(t[key]));
            b.onclick = () => void send({ type, trackID: t.id, [key]: !t[key] }, t.id);
            if (label === 'R')
                b.classList.add('arm');
            b.disabled = client.role === 'viewer' || client.state !== 'connected';
            buttons.append(b);
        }
        const a = document.createElement('button');
        a.textContent = 'A';
        a.setAttribute('aria-label', `Show ${t.name} automation`);
        a.onclick = () => { mirror.view.selectedTrack = t.id; $('automation-editor').hidden = !$('automation-editor').hidden; renderAutomation(); updateSelection(); };
        buttons.append(a);
        title.append(buttons);
        row.append(title);
        const sliders = document.createElement('div');
        sliders.className = 'track-sliders';
        for (const [label, key, min, max, step, type] of [['Vol', 'gain', 0, 1.5, 0.01, 'setTrackGain'], ['Pan', 'pan', -1, 1, 0.01, 'setTrackPan']] as const) {
            const l = document.createElement('label');
            l.textContent = label;
            const input = document.createElement('input');
            input.type = 'range';
            input.min = String(min);
            input.max = String(max);
            input.step = String(step);
            input.value = String(t[key]);
            input.setAttribute('aria-label', `${t.name} ${label === 'Vol' ? 'volume' : 'pan'}`);
            let writer:GestureWriter<number>|undefined,gestureID='',heldKey=false;
            const target={kind:key==='gain'?'volume' as const:'pan' as const},touchControl=new TouchControlBinding(touch,t.id,target),normalized=()=>key==='gain'?Number(input.value)/1.5:(Number(input.value)+1)/2;
            const begin=()=>{editingControls=true;gestureID=randomUUID().toUpperCase();writer=new GestureWriter<number>(async value=>{const current=mirror.snapshot?.session.tracks.find(track=>track.id===t.id);if(!current)return false;return !!await send({type:'updateTrackControls',controls:{...trackControls(current),[key]:value},name:key==='gain'?'Track volume':'Track pan',gestureID},t.id);});};
            input.onpointerdown=()=>{editingControls=true;touchControl.pointerDown();if(!touch.armed(t.id,target))begin();else writer=undefined;};
            input.onkeydown=e=>{if(!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End','PageUp','PageDown'].includes(e.key))return;heldKey=true;editingControls=true;touchControl.keyDown(e.key,e.repeat);if(!e.repeat&&!touch.armed(t.id,target))begin();};
            input.onkeyup=()=>{touchControl.keyUp(normalized());heldKey=false;editingControls=false;};
            input.oninput=()=>{editingControls=true;if(!touchControl.input(normalized())){if(!writer)begin();writer?.offer(Number(input.value));}};
            input.onchange=()=>{if(!touchControl.change(normalized())){if(!writer)begin();writer?.offer(Number(input.value));}editingControls=heldKey||touchControl.active;};
            input.onpointerup=()=>{touchControl.pointerUp(normalized());if(!touch.armed(t.id,target))writer?.offer(Number(input.value));editingControls=false;};
            input.onpointercancel=()=>{touchControl.cancel();writer?.cancel();editingControls=false;};input.onblur=()=>{touchControl.blur();editingControls=false;heldKey=false;writer=undefined;};
            input.disabled = client.role === 'viewer' || client.state !== 'connected';
            l.append(input);
            sliders.append(l);
        }
        row.append(sliders);
        const route = document.createElement('div');
        route.className = 'track-route';
        const device = document.createElement('select');
        device.setAttribute('aria-label', `${t.name} input device`);
        routeOptions(device, s.devices, 'input', t.inputDeviceUID, true);
        const channel = document.createElement('select');
        channel.className = 'channel';
        channel.setAttribute('aria-label', `${t.name} input channel`);
        const count = s.devices.find(d => d.uid === t.inputDeviceUID || (t.inputDeviceUID === undefined && d.id === s.inputDeviceID))?.inputChannels ?? 1;
        for (let i = 0; i < count; i++)
            channel.add(new Option(String(i + 1), String(i)));
        channel.value = String(t.inputChannel ?? 0);
        const update = () => void send({ type: 'setTrackInput', trackID: t.id, deviceUID: device.value || undefined, channel: Number(channel.value), stereo: t.stereoInput ?? false }, t.id);
        device.onchange = update;
        channel.onchange = update;
        device.disabled = channel.disabled = client.role === 'viewer' || client.state !== 'connected';
        const stereo=document.createElement('button');stereo.textContent=t.stereoInput?'2ch':'1ch';stereo.title='Mono / stereo input';stereo.setAttribute('aria-pressed',String(t.stereoInput ?? false));stereo.disabled=client.role==='viewer'||client.state!=='connected'||(t.inputChannel ?? 0)+1>=count&&!t.stereoInput;stereo.onclick=()=>void send({type:'setTrackInput',trackID:t.id,deviceUID:device.value||undefined,channel:Number(channel.value),stereo:!t.stereoInput},t.id);route.append(device, channel,stereo);
        row.append(route);
        container.append(row);
    }
}
function renderSession() {
    const s = mirror.snapshot;
    if (!s)
        return;
    $('session-title').textContent = s.session.name;
    $('revision').textContent = `Revision ${s.revision}`;
    const tempo = $<HTMLInputElement>('tempo');
    if (document.activeElement !== tempo)
        tempo.value = String(s.session.timing.tempo);
    const meter = $<HTMLSelectElement>('meter'), value = `${s.session.timing.beatsPerBar}/${s.session.timing.beatUnit}`;
    if (![...meter.options].some(o => o.value === value))
        meter.add(new Option(value, value));
    meter.value = value;
    routeOptions($('input-device'), s.devices, 'input', s.inputDeviceID);
    routeOptions($('output-device'), s.devices, 'output', s.outputDeviceID);
    $('peers').replaceChildren(...s.peers.map(peer => { const p = document.createElement('span'); p.textContent = `${peer.name} · ${peer.role}${peer.id.toUpperCase() === client.clientID ? ' · you' : ''}`; return p; }));
    renderTrackControls();
    updateSelection();
    syncNavigation();
    timeline.invalidate();
    setStatus(s.status || 'Connected.');
    configureCreative();
    updatePermissions();
    pitch.refresh();
    master.refresh();
    effectWorkspace.refresh();automationWrite.metadataChanged();automation.liveRefresh();writeReview.refresh();
}
function updatePermissions() { configureCreative(); const connected = client.state === 'connected' && sessionReady, canEdit = connected && client.role !== 'viewer'; for (const id of ['add-track', 'import', 'designer', 'beats', 'pitch', 'master', 'undo', 'redo', 'split', 'combine', 'delete', 'save', 'export', 'copy', 'paste', 'cut', 'copy-track', 'copy-track-automation', 'trim-range', 'tempo', 'meter', 'input-device', 'output-device'])
    $<HTMLButtonElement>(id).disabled = !canEdit; $('projects').toggleAttribute('disabled', !connected); arranging.render(); for (const id of ['play', 'record', 'record-setup', 'start', 'loop', 'clear-loop'])
    $<HTMLButtonElement>(id).disabled = !connected || client.role !== 'operator'; }
client.addEventListener('connection', e => { revisionEvents.dispatchEvent(new Event('connection')); const d = (e as CustomEvent).detail; $('connection-label').textContent = d.state === 'connected' ? client.serverName : d.state === 'reconnecting' ? 'Reconnecting…' : d.state === 'authenticating' ? 'Authenticating…' : d.state === 'connecting' ? 'Connecting…' : 'Connect'; $('connection-dot').className = `dot ${d.state}`; if (d.message)
    setStatus(d.message); if (d.state !== 'connected') {
    automationWrite.disconnected();touch.reset();automationSignature='';
    waveformLoader.reset();
    assembler.cancel();
    sessionReady = false;
    pendingDeltas.length = 0;
    pendingDeltaBytes = 0;
    snapshotOverlays.reset();
} updatePermissions(); timeline.invalidate(); if (d.state === 'connected')
    timeline.resetWaveRequests(); });
client.addEventListener('capabilities', () => { rackSignature = ''; automationSignature = ''; configureCreative(); if (mirror.snapshot && typeof client.capabilities.sampleRate === 'number')
    mirror.snapshot.sampleRate = client.capabilities.sampleRate; scheduleRender(); });
client.addEventListener('automationreceipt',e=>{const reply=(e as CustomEvent).detail;const retained=client.retainedAutomationRequests.find(r=>r.id===reply.requestID.toUpperCase());if(!retained)return;const command=JSON.parse(retained.frame).command;if(command.intent?.operation==='automation.writeEnd'&&reply.status==='accepted'&&reply.payload){const update=payload<RemoteAutomationWriteLeaseUpdate>(reply.payload);validateModel(update,'RemoteAutomationWriteLeaseUpdate');automationWrite.update(update);}});
client.addEventListener('conflict', e => setStatus((e as CustomEvent).detail.message));
client.addEventListener('snapshotpage', e => { const message = (e as CustomEvent).detail; try {
    if (message.kind === 'snapshotBegin') {
        assembler.begin(message);
        sessionReady = false;
        updatePermissions();
    }
    else if (message.kind === 'snapshotPage')
        assembler.page(message);
    else {
        const snapshot = assembler.end(message);
        validateModel(snapshot, 'RemoteSnapshot');
        if (!validateSnapshot(snapshot))
            throw new Error('Invalid assembled session.');
        if (mirror.snapshot?.serverID !== snapshot.serverID)
            cache.clear();
        const pinnedRevision=snapshot.revision;
        snapshot.sampleRate=typeof client.capabilities.sampleRate==='number'?client.capabilities.sampleRate:undefined;mirror.applySnapshot(snapshot, performance.now());
        for (const delta of pendingDeltas)
            if (delta.revision > snapshot.revision)
                mirror.applyDelta(delta);
        pendingDeltas.length = 0;
        pendingDeltaBytes = 0;
        snapshotOverlays.apply(mirror,pinnedRevision);
        sessionReady = true;
        scheduleRender();
    }
}
catch (error) {
    assembler.cancel();
    setStatus(error instanceof Error ? error.message : 'Snapshot failed.');
    client.disconnect();
} });
client.addEventListener('serverevent', e => {
    const event = (e as CustomEvent).detail as Record<string, unknown>;
    try {
        switch (event.type) {
            case 'automationWrite':automationWrite.update(event.update as RemoteAutomationWriteLeaseUpdate);break;
            case 'snapshot': {
                const value = event.snapshot;
                if (!validateSnapshot(value))
                    throw new Error('Invalid server session metadata.');
                if (mirror.snapshot?.serverID !== value.serverID)
                    cache.clear();
                value.sampleRate=typeof client.capabilities.sampleRate==='number'?client.capabilities.sampleRate:undefined;snapshotOverlays.install(mirror,value,performance.now());
                sessionReady = true;
                for (const asset of value.assets)
                    cache.put(asset);
                scheduleRender();
                break;
            }
            case 'delta':
                if (!validateDelta(event.delta))
                    throw new Error('Invalid session delta.');
                if (assembler.assembling) {
                    pendingDeltaBytes += new TextEncoder().encode(JSON.stringify(event.delta)).length;
                    if (pendingDeltas.length >= 128 || pendingDeltaBytes > 8 * 1024 * 1024)
                        throw new Error('Too many edits during snapshot; reconnect.');
                    pendingDeltas.push(event.delta as Delta);
                    if ((event.delta as Delta).status !== undefined)
                        snapshotOverlays.retainDeltaStatus((event.delta as Delta).status!, (event.delta as Delta).revision);
                    break;
                }
                mirror.applyDelta(event.delta as Delta);
                for (const asset of (event.delta as Delta).assets)
                    cache.put(asset);
                scheduleRender();
                break;
            case 'transport':
                if (!validateTransport(event.transport))
                    throw new Error('Invalid transport metadata.');
                if (assembler.assembling || !sessionReady)
                    snapshotOverlays.retainTransport(event.transport, performance.now());
                mirror.updateTransport(event.transport as Transport, performance.now());
                $('play').textContent = mirror.snapshot?.transport.isPlaying ? 'Ⅱ Pause' : '▶ Play';
                $('record').textContent=mirror.snapshot?.transport.isRecording?'■ Finish':'● Record';
                timeline.invalidate();
                break;
            case 'devices':
                if (!validateDevices(event.devices))
                    throw new Error('Invalid server devices.');
                if (assembler.assembling || !sessionReady)
                    snapshotOverlays.retainDevices(event.devices,Number(event.inputID),Number(event.outputID));
                if (mirror.snapshot) {
                    mirror.snapshot.devices = event.devices as Device[];
                    mirror.snapshot.inputDeviceID = Number(event.inputID);
                    mirror.snapshot.outputDeviceID = Number(event.outputID);
                    scheduleRender();
                }
                break;
            case 'presence':
                if (assembler.assembling || !sessionReady)
                    snapshotOverlays.retainPresence(event.peers as Snapshot['peers']);
                if (mirror.snapshot) {
                    mirror.snapshot.peers = event.peers as Snapshot['peers'];
                    scheduleRender();
                }
                break;
            case 'status':
                if (assembler.assembling || !sessionReady)
                    snapshotOverlays.retainStatus(String(event.message));
                if (mirror.snapshot)mirror.snapshot.status=String(event.message);
                setStatus(String(event.message));
                break;
            case 'waveform':
                if (!validateAsset(event.asset))
                    throw new Error('Invalid waveform.');
                cache.put(event.asset as Asset);
                timeline.waveformReceived();
                break;
            case 'effectMeters':
                { const meters=event.metersByEffect as Record<string,EffectMeterSnapshot>;if(Object.keys(meters).length>128)throw Error('Too many insert meter entries.');for(const value of Object.values(meters))validateModel(value,'EffectMeterSnapshot');effectWorkspace.readings(meters); }
                break;
            case 'meters': {
                const peak = Math.min(1, Math.max(Number(event.left) || 0, Number(event.right) || 0));
                $('meter-bar').setAttribute('aria-valuenow', String(peak));
                ($('meter-bar').firstElementChild as HTMLElement).style.width = `${peak * 100}%`;
                break;
            }
            case 'midiSources':case 'midiNote':case 'midiController':case 'midiAllNotesOff':
                creative.serverMIDI(event as Parameters<CreativeWorkspaces['serverMIDI']>[0]);break;
            case 'projects':
                projects.projects(event.projects as string[]);
                break;
            case 'history':
                $<HTMLButtonElement>('undo').disabled = !event.canUndo;
                $<HTMLButtonElement>('redo').disabled = !event.canRedo;
                break;
            case 'audition':
                creative.audition(event as unknown as {
                    kind: string;
                    ownerID?: string;
                    playing: boolean;
                    preparing: boolean;
                    position: number;
                });
                break;
            case 'job':
                setStatus(String(event.message));
                break;
        }
    }
    catch (error) {
        setStatus(error instanceof Error ? error.message : 'Session update failed.');
        void client.command({ type: 'snapshot' });
    }
});
new ResizeObserver(() => renderTrackControls()).observe($('track-controls'));
$('timeline').addEventListener('verticalscroll', e => { vertical = (e as CustomEvent).detail; renderTrackControls(); });
$<HTMLSelectElement>('effects-scope').onchange = () => { rackSignature = ''; renderEffects(); };
$('pitch').onclick = () => pitch.open();
$('master').onclick = () => master.open();
$('designer').onclick = () => creative.open('designer');
$('beats').onclick = () => creative.open('beats');
$('connection').onclick = () => openWorkspaceDialog($<HTMLDialogElement>('connect-dialog'));
$('close-connect').onclick = () => $<HTMLDialogElement>('connect-dialog').close();
let trustedLANMode=false;
const readRememberedPairing=()=>{try{return loadPairing(localStorage,location.origin);}catch{return undefined;}};
$('forget-pairing').onclick = () => {try{forgetPairing(localStorage);}catch{}client.forgetCredential();$<HTMLInputElement>('pairing-code').value='';$('connect-error').textContent='Pairing forgotten. Create a fresh code to connect again.';};
client.addEventListener('authenticatedcredential',e=>{if(!trustedLANMode)return;try{if($<HTMLInputElement>('remember-pairing').checked)savePairing(localStorage,{endpoint:(e as CustomEvent).detail.endpoint,name:(e as CustomEvent).detail.name,credential:(e as CustomEvent).detail.credential});else forgetPairing(localStorage);}catch{$('connect-error').textContent='Connected, but this browser could not save pairing.';}});
$('disconnect').onclick = () => { client.disconnect(); $<HTMLDialogElement>('connect-dialog').close(); };
$<HTMLFormElement>('connect-form').onsubmit = event => { event.preventDefault(); void (async () => { try {
    const file = $<HTMLInputElement>('credential-file').files?.[0];
    const code = $<HTMLInputElement>('pairing-code').value.trim();
    const remembered=trustedLANMode&&$<HTMLInputElement>('remember-pairing').checked ? readRememberedPairing():undefined;
    const matching=remembered?.endpoint===$<HTMLInputElement>('endpoint').value&&remembered?.name===$<HTMLInputElement>('client-name').value ? remembered:undefined;
    if (!code && !matching && (!file || file.size > 16384))
        throw new Error('Enter a pairing code or choose a valid pairing file.');
    const credential = code ? { code } : file ? validateCredential(JSON.parse(await file.text())) : matching!.credential;
    await client.connect($<HTMLInputElement>('endpoint').value, credential, $<HTMLInputElement>('client-name').value);
    $<HTMLDialogElement>('connect-dialog').close();
}
catch (error) {
    $('connect-error').textContent = error instanceof Error ? error.message : 'Unable to connect.';
} })(); };
void fetch('/soundroom-config.json', { cache: 'no-store' }).then(r => r.ok ? r.json() : undefined).then(config => { if (config?.wssURL) {
    const url = new URL(config.wssURL);
    if (config.trustedLAN === true && location.protocol === 'http:' && url.protocol === 'ws:' && url.origin.replace(/^ws:/,'http:') === location.origin && url.pathname === '/control') {
        client.allowTrustedLAN(location.origin);trustedLANMode=true;$('remember-pairing-controls').hidden=false;
        $('connection-mode').textContent = 'Trusted LAN · unencrypted connection. Pairing and access roles are required.';
        $<HTMLInputElement>('endpoint').value = url.href;
        const remembered=readRememberedPairing();if(remembered){$<HTMLInputElement>('client-name').value=remembered.name;void client.connect(remembered.endpoint,remembered.credential,remembered.name).catch(error=>setStatus(error instanceof Error?error.message:'Saved pairing failed.'));}
    }
    if (url.protocol === 'wss:' && url.hostname === location.hostname)
        $<HTMLInputElement>('endpoint').value = url.href;
} }).catch(() => { });
$('play').onclick = () => { if (mirror.snapshot?.transport.isPlaying)
    void (async () => { await editor('transport.pause'); if (mirror.snapshot) {
        mirror.view.cursor = mirror.snapshot.transport.frame;
        updateSelection();
        timeline.invalidate();
    } })();
else
    void (async () => { if (await send({ type: 'seek', frame: mirror.view.cursor }))
        await send({ type: 'play' }); })(); };
$('record').onclick = () => void recording.record();
$('record-setup').onclick=()=>recording.open();
$('start').onclick = () => { mirror.view.cursor = 0; mirror.view.offset = 0; void send({ type: 'seek', frame: 0 }); updateSelection(); timeline.invalidate(); };
$('add-track').onclick = () => void send({ type: 'addTrack', name: `Audio ${(mirror.snapshot?.session.tracks.length ?? 0) + 1}` });
$('undo').onclick = () => void send({ type: 'undo' });
$('redo').onclick = () => void send({ type: 'redo' });
$('split').onclick = () => { const clipID = [...mirror.view.selectedClips][0]; if (clipID)
    void send({ type: 'splitClip', trackID: mirror.view.selectedTrack, clipID, frame: mirror.view.cursor }, mirror.view.selectedTrack); };
$('combine').onclick = () => void editor('clips.combine');
$('delete').onclick = () => { if (mirror.view.selectedClips.size)
    void send({ type: 'deleteClips', trackID: mirror.view.selectedTrack, clipIDs: [...mirror.view.selectedClips] }, mirror.view.selectedTrack); };
$('zoom-in').onclick = () => { timeline.zoom(1.5); syncNavigation(); };
$('zoom-out').onclick = () => { timeline.zoom(1 / 1.5); syncNavigation(); };
$('fit').onclick = () => { const s = mirror.snapshot; if (!s)
    return; const end = Math.max(mirror.sampleRate * 10, ...s.session.tracks.flatMap(t => t.clips.map(c => c.start + c.length))); mirror.view.offset = 0; mirror.view.pixelsPerSecond = Math.max(2, $('timeline').clientWidth / (end / mirror.sampleRate)); timeline.invalidate(); syncNavigation(); };
$('snap').onclick = () => { mirror.view.snap = !mirror.view.snap; updateSelection(); };
$('follow').onclick = () => { mirror.view.follow = !mirror.view.follow; updateSelection(); };
$<HTMLInputElement>('scroll').oninput = e => { mirror.view.offset = Number((e.target as HTMLInputElement).value); mirror.view.follow = false; updateSelection(); timeline.invalidate(); };
$<HTMLSelectElement>('timebase').onchange = e => { mirror.view.timebase = (e.target as HTMLSelectElement).value as 'time' | 'beats'; updateSelection(); timeline.invalidate(); };
$<HTMLInputElement>('tempo').onchange = e => void editor('session.timing', { tempo: Number((e.target as HTMLInputElement).value) });
$<HTMLSelectElement>('meter').onchange = e => { const [beatsPerBar, beatUnit] = (e.target as HTMLSelectElement).value.split('/').map(Number); void editor('session.timing', {}, { beatsPerBar: beatsPerBar!, beatUnit: beatUnit! }); };
for (const id of ['cursor', 'range-start', 'range-end'])
    $<HTMLInputElement>(id).onchange = () => { try {
        const s = mirror.snapshot;
        if (!s)
            return;
        const frame = parsePosition($<HTMLInputElement>(id).value, s.session.timing, mirror.sampleRate, mirror.view.timebase);
        if (id === 'cursor')
            mirror.view.cursor = frame;
        else {
            const range: [
                number,
                number
            ] = mirror.view.range ? [...mirror.view.range] : [mirror.view.cursor, mirror.view.cursor];
            range[id === 'range-start' ? 0 : 1] = frame;
            if (range[0] > range[1])
                throw new Error('Selection end must follow its start.');
            mirror.view.range = range;
        }
        updateSelection();
        timeline.invalidate();
    }
    catch (error) {
        setStatus(error instanceof Error ? error.message : 'Invalid position.');
    } };
$('loop').onclick = () => { if (mirror.view.range && mirror.view.range[1] > mirror.view.range[0])
    void send({ type: 'setLoop', start: mirror.view.range[0], end: mirror.view.range[1] }); };
$('clear-loop').onclick = () => void send({ type: 'setLoop' });
$<HTMLSelectElement>('input-device').onchange = () => { const d = mirror.snapshot?.devices.find(d => String(d.id) === $<HTMLSelectElement>('input-device').value); if (d)
    void send({ type: 'setDevices', inputUID: d.uid }); };
$<HTMLSelectElement>('output-device').onchange = () => { const d = mirror.snapshot?.devices.find(d => String(d.id) === $<HTMLSelectElement>('output-device').value); if (d)
    void send({ type: 'setDevices', outputUID: d.uid }); };
$('projects').onclick = () => void projects.open();
$('copy').onclick = () => arranging.copy();
$('cut').onclick = () => arranging.copy(true);
$('paste').onclick = () => void arranging.paste();
$('copy-track').onclick = () => arranging.copyWholeTrack();
$('copy-track-automation').onclick = () => arranging.copyWholeTrack(true);
$('trim-range').onclick = () => void arranging.trimSelection();
$('save').onclick = () => { const name = prompt('Save project as', mirror.snapshot?.session.name ?? 'Untitled'); if (name?.trim())
    void send({ type: 'saveProject', name: name.trim() }); };
$('import').onclick = () => $<HTMLInputElement>('audio-file').click();
$('cancel-transfer').onclick = () => transfer?.abort();
function progress(value: number, message: string) { $<HTMLProgressElement>('progress').value = value; setStatus(message); }
async function transferJob(job: (signal: AbortSignal) => Promise<void>) { if (transfer) {
    setStatus('A transfer is already running.');
    return;
} transfer = new AbortController(); $('progress').hidden = false; $('cancel-transfer').hidden = false; try {
    await job(transfer.signal);
}
catch (error) {
    setStatus(error instanceof Error ? error.message : 'Transfer failed.');
}
finally {
    transfer = undefined;
    $('progress').hidden = true;
    $('cancel-transfer').hidden = true;
} }
$<HTMLInputElement>('audio-file').onchange = () => { const file = $<HTMLInputElement>('audio-file').files?.[0]; if (file)
    void transferJob(signal => importAudio(client, file, mirror.view.selectedTrack || undefined, mirror.view.cursor, progress, signal)); $<HTMLInputElement>('audio-file').value = ''; };
interface SaveFileWindow extends Window {
    showSaveFilePicker?: (options: {
        suggestedName: string;
        types: {
            description: string;
            accept: Record<string, string[]>;
        }[];
    }) => Promise<{
        createWritable: () => Promise<ExportSink>;
    }>;
}
$('export').onclick = () => void (async () => { let sink: ExportSink | undefined; try {
    const picker = (window as SaveFileWindow).showSaveFilePicker;
    if (picker) {
        const handle = await picker.call(window, { suggestedName: `${mirror.snapshot?.session.name ?? 'Mix'}.wav`, types: [{ description: 'WAV audio', accept: { 'audio/wav': ['.wav'] } }] });
        sink = await handle.createWritable();
    }
    await transferJob(async (signal) => { const result = await downloadMix(client, mirror.snapshot?.session.name ?? 'Mix', progress, signal, sink); if (result.blob) {
        const url = URL.createObjectURL(result.blob), a = document.createElement('a');
        a.href = url;
        a.download = result.name;
        a.click();
        setTimeout(() => URL.revokeObjectURL(url), 60000);
    } setStatus(result.written ? 'Export saved to disk.' : 'Export downloaded.'); });
}
catch (error) {
    setStatus(error instanceof Error ? error.message : 'Export cancelled.');
} })();
document.addEventListener('keydown', event => { const target = event.target as HTMLElement; if (!acceptWorkspaceShortcut(event) || target?.closest?.('dialog'))
    return; if ((event.metaKey || event.ctrlKey) && ['c', 'x', 'v', 't', 'l', 's'].includes(event.key.toLowerCase())) {
    event.preventDefault();
    if (event.key.toLowerCase() === 'c')
        arranging.copy();
    else if (event.key.toLowerCase() === 'x')
        arranging.copy(true);
    else if (event.key.toLowerCase() === 'v')
        void arranging.paste();
    else if (event.key.toLowerCase() === 't')
        void arranging.trimSelection();
    else if (event.key.toLowerCase() === 'l')
        void arranging.align();
    else if (!$<HTMLButtonElement>('save').disabled)
        $('save').click();
}
else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
    event.preventDefault();
    void arranging.nudge((event.key === 'ArrowRight' ? 1 : -1) * Math.round(mirror.sampleRate * (event.shiftKey ? 1 : 0.01)));
}
else if (event.code === 'Space') {
    event.preventDefault();
    if (!$<HTMLButtonElement>('play').disabled)
        $('play').click();
}
else if (event.key === '=' || event.key === '+') {
    event.preventDefault();
    $('zoom-in').click();
}
else if (event.key === '-') {
    event.preventDefault();
    $('zoom-out').click();
}
else if (event.key === 'Delete' || event.key === 'Backspace') {
    event.preventDefault();
    if (!$<HTMLButtonElement>('delete').disabled)
        $('delete').click();
}
else if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'z') {
    event.preventDefault();
    const id = event.shiftKey ? 'redo' : 'undo';
    if (!$<HTMLButtonElement>(id).disabled)
        $(id).click();
}
else if (event.key.toLowerCase() === 's' && !event.metaKey && !event.ctrlKey) {
    event.preventDefault();
    if (!$<HTMLButtonElement>('split').disabled)
        $('split').click();
} });
function clock() { const s = mirror.snapshot; if (s)
    $('position').textContent = formatPosition(s.transport.isPlaying ? mirror.displayedFrame(performance.now()) : mirror.view.cursor, s.session.timing, mirror.sampleRate, mirror.view.timebase); requestAnimationFrame(clock); }
requestAnimationFrame(clock);
updatePermissions();
