import type { Track } from './model.js';
import { ownAcceptedTrackRevision } from './gesture-revision.js';
export interface WorkspaceSourceAccess {
    track(id: string): Track | undefined;
    version(id: string): number | undefined;
    sessionRevision(): number | undefined;
    epoch?(): number;
}
/** A private draft owns its opening destination; private navigation and later metadata cannot retarget it. */
export class WorkspaceSource {
    trackRevision?: number;
    readonly sessionRevision?: number;
    readonly connectionEpoch?: number;
    constructor(readonly trackID: string | undefined, readonly clipID: string | undefined, readonly frame: number, private access?: WorkspaceSourceAccess) {
        this.trackRevision = trackID ? access?.version(trackID) : undefined;
        this.sessionRevision = access?.sessionRevision(); this.connectionEpoch = access?.epoch?.();
    }
    ready() {
        if (!this.access) return true;
        if (this.connectionEpoch !== this.access.epoch?.()) return false;
        if (!this.trackID) return this.sessionRevision !== undefined && this.access.sessionRevision() === this.sessionRevision;
        const track = this.access.track(this.trackID);
        return !!track && this.trackRevision !== undefined && this.access.version(this.trackID) === this.trackRevision && (!this.clipID || track.clips.some(c => c.id === this.clipID));
    }
    accepted(receipt: unknown) {
        if (!this.access) return !!receipt;
        if (this.connectionEpoch !== this.access.epoch?.()) return false;
        if (!this.trackID) { const revision=receipt&&typeof receipt==='object'?(receipt as {revision?:number}).revision:undefined;return Number.isSafeInteger(revision)&&this.sessionRevision!==undefined&&revision!>=this.sessionRevision&&revision===this.access.sessionRevision(); }
        if (this.trackRevision === undefined || !this.access.track(this.trackID)) return false;
        const next = ownAcceptedTrackRevision(this.trackRevision, this.access.version(this.trackID), receipt);
        if (next === undefined) return false; this.trackRevision = next; return true;
    }
}
