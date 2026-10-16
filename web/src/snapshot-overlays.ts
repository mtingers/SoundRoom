import type { Device, Peer, Snapshot, Transport } from './model.js';
import { SessionMirror } from './model.js';
/** Host events can arrive after snapshotBegin but describe newer state than its
 * immutable metadata pages. Keep one bounded latest value per overlay, including
 * events received before the first snapshot exists. Never retain event history. */
export class SnapshotOverlays {
    private transport?: { value: Transport; received: number };
    private devices?: { value: Device[]; inputID: number; outputID: number };
    private peers?: Peer[];
    private status?: {value:string;sequence:number};
    private deltaStatus?: {value:string;sequence:number;revision:number};
    private sequence=0;
    private bytes = { devices: 0, peers: 0, status: 0,deltaStatus:0 };
    private bound(kind: keyof SnapshotOverlays['bytes'], value: unknown) {
        const size = new TextEncoder().encode(JSON.stringify(value)).length;
        const total = Object.values(this.bytes).reduce((a,b)=>a+b,0) - this.bytes[kind] + size;
        if (total > 1024 * 1024)
            throw Error('Too much live metadata during snapshot; reconnect.');
        this.bytes[kind] = size;
    }
    retainTransport(value: Transport, received: number) { this.transport = { value, received }; }
    retainDevices(value: Device[], inputID: number, outputID: number) {
        if (value.length > 256)
            throw Error('Too many host devices during snapshot.');
        this.bound('devices', value);
        this.devices = { value, inputID, outputID };
    }
    retainPresence(value: Peer[]) {
        if (value.length > 256)
            throw Error('Too many connected clients during snapshot.');
        this.bound('peers', value);
        this.peers = value;
    }
    retainStatus(value: string) { this.bound('status', value); this.status = {value,sequence:++this.sequence}; }
    retainDeltaStatus(value:string,revision:number) {
        if(this.deltaStatus&&revision<this.deltaStatus.revision)return;
        this.bound('deltaStatus',value);this.deltaStatus={value,revision,sequence:++this.sequence};
    }
    apply(mirror: SessionMirror,pinnedRevision=mirror.snapshot?.revision??-1) {
        const snapshot = mirror.snapshot;
        if (!snapshot)
            return;
        if (this.devices) {
            snapshot.devices = this.devices.value;
            snapshot.inputDeviceID = this.devices.inputID;
            snapshot.outputDeviceID = this.devices.outputID;
        }
        if (this.peers !== undefined)
            snapshot.peers = this.peers;
        const status=this.deltaStatus&&this.deltaStatus.revision>pinnedRevision&&this.deltaStatus.sequence>(this.status?.sequence??-1)?this.deltaStatus:this.status;
        if (status !== undefined)
            snapshot.status = status.value;
        if (this.transport)
            mirror.updateTransport(this.transport.value, this.transport.received);
        this.reset();
    }
    install(mirror: SessionMirror, snapshot: Snapshot, received: number) {
        mirror.applySnapshot(snapshot, received);
        this.apply(mirror);
    }
    reset() { this.transport = undefined; this.devices = undefined; this.peers = undefined; this.status = undefined; this.deltaStatus=undefined;this.sequence=0;this.bytes = { devices: 0, peers: 0, status: 0,deltaStatus:0 }; }
}
