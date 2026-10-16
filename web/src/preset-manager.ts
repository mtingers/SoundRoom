import {randomUUID} from './crypto.js';
import { openWorkspaceDialog } from './input-focus.js';
import type { EffectMetadata } from './effects.js';
import type { RealtimeEffect } from './generated/models.js';
import { EffectLibrary, encodeEffectPresets, encodeEffectChains, presetFromEffect, prepareEffectChain, type SavedEffectPreset, type SavedEffectChain } from './effect-presets.js';
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, text?: string) => { const e = document.createElement(tag); if (text) e.textContent = text; return e; };
export function downloadPreset(text: string, name: string) {
    const url = URL.createObjectURL(new Blob([text], { type: 'application/json' })), link = el('a');
    link.href = url; link.download = name.replace(/[\/\\\u0000-\u001f]/g, '_'); link.click(); setTimeout(() => URL.revokeObjectURL(url), 30_000);
}
/** Private library management. Only Apply authors server state; import/save/delete never change the project. */
export class PresetManager {
    private dialog = el('dialog');
    private body = el('div');
    private status = el('p');
    private kind: 'preset' | 'chain' = 'preset';
    private selected = '';
    private busy = false;
    constructor(private library: EffectLibrary, private metadata: () => EffectMetadata[], private effect: () => RealtimeEffect | undefined, private rack: () => RealtimeEffect[], private canApply: () => boolean, private applyPreset: (preset: SavedEffectPreset) => Promise<boolean>, private applyChain: (chain: SavedEffectChain) => Promise<boolean>) {
        this.dialog.className = 'preset-manager'; this.dialog.setAttribute('aria-label', 'Personal effect and chain presets');
        const header = el('div'); header.className = 'workspace-toolbar'; const done = el('button', 'Done'); done.onclick = () => this.dialog.close();
        header.append(el('h2', 'Personal presets'), done); this.status.setAttribute('role', 'status'); this.status.className = 'workspace-status';
        this.dialog.append(header, this.body, this.status); document.body.append(this.dialog);
        this.library.addEventListener('change', () => { if (this.dialog.open) this.render(); });
    }
    open(kind: 'preset' | 'chain' = 'preset') { this.kind = kind; this.selected = ''; this.status.textContent = this.library.error ?? 'Saved in this browser. Export a file to use these settings in another client.'; this.render(); openWorkspaceDialog(this.dialog); }
    private action(label: string, callback: () => void | Promise<void>, enabled = true) { const button = el('button', label); button.disabled = this.busy || !enabled; button.onclick = () => { try { void Promise.resolve(callback()).catch(e => this.status.textContent = String(e)); } catch (e) { this.status.textContent = String(e); } }; return button; }
    private render() {
        this.body.replaceChildren();
        const tabs = el('div'); tabs.className = 'effect-actions';
        for (const [label, kind] of [['Effects', 'preset'], ['Chains', 'chain']] as const) { const button = this.action(label, () => { this.kind = kind; this.selected = ''; this.render(); }); button.setAttribute('aria-pressed', String(this.kind === kind)); tabs.append(button); }
        this.body.append(tabs);
        const list = el('select'); list.size = 8; list.className = 'preset-list'; list.setAttribute('aria-label', this.kind === 'preset' ? 'Saved effect presets' : 'Saved insert chains');
        const entries = this.kind === 'preset' ? this.library.presets() : this.library.chains();
        for (const p of entries) list.add(new Option(this.kind === 'preset' ? `${p.name} · ${this.metadata().find(m => m.id === (p as SavedEffectPreset).kind)?.name ?? (p as SavedEffectPreset).kind}${(p as SavedEffectPreset).legacyAmount != null ? ' · legacy' : ''}` : `${p.name} · ${(p as SavedEffectChain).effects.length} inserts${(p as SavedEffectChain).includesAutomation ? ' + automation' : ''}`, p.id));
        list.value = this.selected; list.onchange = () => { this.selected = list.value; this.render(); this.body.querySelector<HTMLSelectElement>('.preset-list')?.focus(); }; this.body.append(list);
        const current = entries.find(p => p.id === this.selected), naming = el('label', 'Name'), name = el('input'); name.value = current?.name ?? ''; name.setAttribute('aria-label', 'Preset name, up to 80 grapheme characters'); naming.append(name); this.body.append(naming);
        const include = el('input'); include.type = 'checkbox'; include.checked = this.kind === 'chain' && (current as SavedEffectChain | undefined)?.includesAutomation === true;
        if (this.kind === 'chain') { const label = el('label', 'Include automation and manual pitch targets'); label.prepend(include); this.body.append(label); }
        const actions = el('div'); actions.className = 'effect-actions';
        actions.append(this.action('Save current', () => {
            if (this.kind === 'preset') { const source = this.effect(); if (!source) throw Error('Select an insert before saving an effect preset.'); const p = presetFromEffect(name.value, source, this.metadata()); this.library.savePreset(p); this.selected = p.id; }
            else { const chain: SavedEffectChain = { id: randomUUID().toUpperCase(), name: name.value, includesAutomation: include.checked, effects: this.rack() }; chain.effects = prepareEffectChain(chain, this.metadata()); this.library.saveChain(chain); this.selected = chain.id; }
            this.render(); this.status.textContent = 'Saved in this browser.';
        }, !this.library.error), this.action('Replace settings', () => {
            if (!current) return;
            if (this.kind === 'preset') { const source = this.effect(); if (!source || source.kind !== (current as SavedEffectPreset).kind) throw Error('Select an insert of the saved preset’s effect type.'); this.library.savePreset(presetFromEffect(current.name, source, this.metadata()), current.id); }
            else { const chain: SavedEffectChain = { ...(current as SavedEffectChain), includesAutomation: include.checked, effects: this.rack() }; chain.effects = prepareEffectChain(chain, this.metadata()); this.library.saveChain(chain, current.id); }
            this.status.textContent = 'Preset settings replaced; the project is unchanged.';
        }, !!current && !this.library.error), this.action('Rename', () => { if (current) this.library.rename(this.kind, current.id, name.value); }, !!current && !this.library.error), this.action('Delete', () => { if (current && confirm(`Delete “${current.name}” from this browser’s library?`)) { this.library.remove(this.kind, current.id); this.selected = ''; this.render(); this.status.textContent = 'Deleted. Undo library restores this entry.'; } }, !!current && !this.library.error));
        this.body.append(actions);
        const transfer = el('div'); transfer.className = 'effect-actions';
        transfer.append(this.action('Import file…', () => {
            const input = el('input'); input.type = 'file'; input.accept = '.soundroom-effectpresets,.soundroom-effectchain,.json';
            input.onchange = () => { const file = input.files?.[0]; if (!file) return; if (file.size > 4_194_304) { this.status.textContent = 'Preset files are limited to 4 MiB.'; return; } void file.text().then(text => { const imported = this.library.import(text); this.kind = imported.kind; this.selected = ''; this.render(); this.status.textContent = `Imported ${imported.count} ${imported.kind === 'preset' ? 'effect presets' : 'chains'} into this browser.`; }).catch(e => this.status.textContent = String(e)); }; input.click();
        }, !this.library.error), this.action('Export selected', () => { if (!current) return; const text = this.kind === 'preset' ? encodeEffectPresets([current as SavedEffectPreset], this.metadata()) : encodeEffectChains([current as SavedEffectChain], this.metadata()); downloadPreset(text, `${current.name}.${this.kind === 'preset' ? 'soundroom-effectpresets' : 'soundroom-effectchain'}`); this.status.textContent = 'Native preset file exported. Automation and source guard fields remain represented in chain files.'; }, !!current), this.action('Undo library', () => { this.library.undo(); this.status.textContent = 'Library change undone; project Undo is separate.'; }, this.library.canUndo));
        this.body.append(transfer);
        const apply = this.action(this.kind === 'preset' ? 'Apply to insert' : 'Replace insert chain', async () => {
            if (!current) return;
            if (!this.canApply()) { this.status.textContent = 'Applying presets requires an authenticated editor or operator connection.'; return; }
            if (this.kind === 'chain' && !confirm(`Replace the current insert chain with “${current.name}”? Project Undo retains the existing chain.`)) return;
            this.busy = true; this.render();
            try { const accepted = await (this.kind === 'preset' ? this.applyPreset(current as SavedEffectPreset) : this.applyChain(current as SavedEffectChain)); this.status.textContent = accepted ? 'Applied on the hardware server. Project Undo retains the previous settings.' : 'The edit was not accepted. Review the server status before retrying.'; }
            finally { this.busy = false; this.render(); }
        }, !!current && this.canApply() && (this.kind === 'chain' || (current as SavedEffectPreset).kind === this.effect()?.kind));
        if(!this.canApply())apply.title='Applying presets requires an authenticated editor or operator connection.';
        apply.className = 'primary'; this.body.append(apply);
    }
}
