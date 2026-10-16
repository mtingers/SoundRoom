/** Native controls own keyboard activation/editing; DAW shortcuts belong to the surrounding workspace. */
export function ownsShortcutInput(target: EventTarget | null): boolean {
    const element = target as Element | null;
    return !!element?.closest?.('input,textarea,select,button,a[href],summary,[contenteditable]:not([contenteditable="false"]),[role="button"],[role="slider"],[role="combobox"]');
}
export function acceptWorkspaceShortcut(event: Pick<KeyboardEvent, 'target' | 'defaultPrevented' | 'isComposing' | 'code' | 'repeat' | 'preventDefault'>): boolean {
    if (event.defaultPrevented || event.isComposing || ownsShortcutInput(event.target)) return false;
    if (event.code === 'Space' && event.repeat) { event.preventDefault(); return false; }
    return true;
}
/** Virtual instrument keys need down/up semantics; ordinary button click would never release a note. */
export function bindInstrumentKey(button: HTMLButtonElement, press: () => void, release: () => void) {
    let held = false;
    const stop = () => { if (held) { held = false; release(); } };
    button.addEventListener('keydown', event => {
        if (button.disabled || event.isComposing || !['Space', 'Enter'].includes(event.code)) return;
        event.preventDefault(); event.stopPropagation();
        if (!held && !event.repeat) { held = true; press(); }
    });
    button.addEventListener('keyup', event => { if (['Space', 'Enter'].includes(event.code)) { event.preventDefault(); event.stopPropagation(); stop(); } });
    button.addEventListener('blur', stop);
    return stop;
}
export const isItemNavigation = (key: string) => ['Home', 'End', '[', ']'].includes(key);
/** Private chart selection in temporal order. Equal-time items use stable source order; no sorted copy. */
export function navigateTimelineItem<T>(items: readonly T[], selected: T | undefined, key: string, position: (item: T) => number): T | undefined {
    if (!isItemNavigation(key)) return undefined;
    const current = selected === undefined ? -1 : items.indexOf(selected), last = key === 'End' || key === '[';
    let chosen = -1;
    const compare = (a: number, b: number) => position(items[a]!) - position(items[b]!) || a - b;
    for (let i = 0; i < items.length; i++) {
        if (!Number.isFinite(position(items[i]!))) continue;
        if (current >= 0 && (key === '[' ? compare(i, current) >= 0 : key === ']' ? compare(i, current) <= 0 : false)) continue;
        if (chosen < 0 || (last ? compare(i, chosen) > 0 : compare(i, chosen) < 0)) chosen = i;
    }
    return chosen >= 0 ? items[chosen] : current >= 0 ? selected : undefined;
}
const origins = new WeakMap<HTMLDialogElement, HTMLElement | undefined>();
/** Native showModal traps Tab. Remember a usable focus return even when the mixer replaces its opener. */
export function openWorkspaceDialog(dialog: HTMLDialogElement, focus?: HTMLElement) {
    if (dialog.open) return;
    const previous = document.activeElement as HTMLElement | null;
    origins.set(dialog, previous ?? undefined);
    if (!dialog.dataset.focusManaged) {
        dialog.dataset.focusManaged = 'true';
        dialog.addEventListener('close', () => {
            if (dialog.open) return; // A queued close event must not steal focus from a reopened workspace.
            const origin = origins.get(dialog); origins.delete(dialog);
            if (origin?.isConnected && !origin.closest('[inert],dialog:not([open])') && !(origin as HTMLButtonElement).disabled) origin.focus({ preventScroll: true });
            else {
                const parent = [...document.querySelectorAll<HTMLDialogElement>('dialog[open]')].at(-1);
                const fallback = parent?.querySelector<HTMLElement>('canvas[tabindex],button:not(:disabled),input:not(:disabled),select:not(:disabled)') ?? document.querySelector<HTMLElement>('#timeline canvas[tabindex],#play');
                fallback?.focus({ preventScroll: true });
            }
        });
    }
    dialog.showModal();
    if (focus?.isConnected) focus.focus({ preventScroll: true });
    else if (focus) { dialog.tabIndex = -1; dialog.focus({ preventScroll: true }); }
}
