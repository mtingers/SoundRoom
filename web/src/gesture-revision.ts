/** An accepted receipt identifies our target version; a later foreign version must never be adopted. */
export function ownAcceptedTrackRevision(before: number, observed: number | undefined, receipt: unknown, verifiedUnchanged = false): number | undefined {
    if (!receipt || typeof receipt !== 'object' || !Number.isSafeInteger((receipt as { revision?: number }).revision)) return undefined;
    const revision = (receipt as { revision: number }).revision;
    if (revision < before) return undefined;
    // No-op controls do not advance a target even if unrelated session metadata is newer.
    return observed === revision || observed === before && verifiedUnchanged ? observed : undefined;
}
export class TrackGestureRevision {
    private expected: number | undefined;
    private readonly connectionEpoch?:number;
    constructor(readonly trackID: string, private identity: () => string | undefined, private version?: (trackID: string) => number | undefined,private epoch?:()=>number) { this.expected = version?.(trackID);this.connectionEpoch=epoch?.(); }
    get revision() { return this.expected; }
    ready() { return this.connectionEpoch===this.epoch?.()&&this.identity() === this.trackID && (!this.version || this.expected !== undefined && this.version(this.trackID) === this.expected); }
    accepted(receipt: unknown, verifiedUnchanged = false) {
        if (this.connectionEpoch!==this.epoch?.()||this.identity() !== this.trackID) return false;
        if (!this.version) return !!receipt;
        if (this.expected === undefined) return false;
        const revision = ownAcceptedTrackRevision(this.expected, this.version(this.trackID), receipt, verifiedUnchanged);
        if (revision === undefined) return false;
        this.expected = revision; return true;
    }
}
