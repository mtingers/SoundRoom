/** Receipts may use a priority queue ahead of metadata; dependent edits await its revision. */
export function revisionBarrier(target: number, current: () => number, valid: () => boolean, events: EventTarget, timeout = 5000): Promise<boolean> {
    if (!valid())
        return Promise.resolve(false);
    if (current() >= target)
        return Promise.resolve(true);
    return new Promise(resolve => { let complete = false; const finish = (ok: boolean) => { if (complete)
        return; complete = true; clearTimeout(timer); events.removeEventListener('revision', check); events.removeEventListener('connection', check); resolve(ok); }; const check = () => { if (!valid())
        finish(false);
    else if (current() >= target)
        finish(true); }; const timer = setTimeout(() => finish(false), timeout); events.addEventListener('revision', check); events.addEventListener('connection', check); check(); });
}
