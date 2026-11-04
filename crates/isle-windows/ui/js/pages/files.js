// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The Shelf's Files tab (gnome-extension/src/pages/files.js): up to six files parked on the island. Drop files on
// the island to add them; drag a tile into any app, click it to copy the file, double-click to open it. Files the
// Shelf keeps itself (from a phone, a clipboard picture) have ↓ to save them into Downloads.

import {daemon} from '../daemon.js';
import {el, icon, iconButton} from '../widgets.js';

const {invoke} = window.__TAURI__.core;

/** The shelf holds this many files (as Isle enforces). */
export const SHELF_CAPACITY = 6;
/** How long "copied" shows on a tile. */
const COPIED_MS = 1100;
/** A press that moves this far is a drag. */
const DRAG_PX = 5;
const DOUBLE_CLICK_MS = 400;

/** Previews by URI, asked of Windows once while the file stays. */
const previews = new Map();
function previewOf(uri) {
    if (!previews.has(uri))
        previews.set(uri, invoke('shelf_preview', {uri}).catch(() => null));
    return previews.get(uri);
}

export class FilesView {
    constructor() {
        this.node = el('div', 'isle-shelf-files');
        /** Called with a short line for the hint (saved, couldn't save). */
        this.onNote = null;
        this._tiles = el('div', 'isle-shelf-tiles');
        this._emptyTitle = el('div', 'isle-empty-title');
        this._emptyHint = el('div', 'isle-empty-hint');
        this._empty = el('div', 'isle-empty', [icon('isle-shelf', 24, 'isle-empty-icon'), this._emptyTitle, this._emptyHint]);
        this.node.append(this._tiles, this._empty);
        this._tileFor = new Map();
        this._dropping = false;
        this._dropOver = false;
        this._refusal = null;
        this._lastClick = {uri: null, at: 0};
        this.update();
    }

    get items() {
        return daemon.state.shelf ?? [];
    }

    /** A file is on its way: the zone and words say where it will land. */
    set dropping(dropping) {
        this._dropping = dropping;
        this._syncZone();
    }

    /** The file is right over the drop zone. */
    set dropOver(over) {
        this._dropOver = over;
        this._syncZone();
    }

    /** Why the files on their way can't all land ('local', 'room'), or null: the outline turns red. */
    set refusal(refusal) {
        this._refusal = refusal;
        this._syncZone();
    }

    /** A file landed: the outline flashes white and fades. */
    flashLanded() {
        this.node.classList.remove('landed');
        void this.node.offsetWidth;
        this.node.classList.add('landed');
    }

    /** Whether `x`, `y` (CSS px in the window) is over the drop zone. */
    contains(x, y) {
        const box = this.node.getBoundingClientRect();
        return box.width > 0 && x >= box.left && x <= box.right && y >= box.top && y <= box.bottom;
    }

    _syncZone() {
        const items = this.items;
        this.node.classList.toggle('zone', this._dropping || items.length === 0);
        this.node.classList.toggle('near', this._dropping);
        this.node.classList.toggle('over', this._dropping && this._dropOver && !this._refusal);
        this.node.classList.toggle('refused', this._dropping && Boolean(this._refusal));
        let title = 'No files on the shelf', hint = 'Drop files here, then drag them into any app';
        if (this._dropping && this._dropOver && this._refusal === 'local')
            [title, hint] = ['Only files on this computer', 'Links and files online can’t be kept'];
        else if (this._dropping && this._dropOver && this._refusal)
            [title, hint] = ['Too many files', 'The shelf holds six at a time'];
        else if (this._dropping)
            [title, hint] = [this._dropOver ? 'Release to add it' : 'Drop to add it', 'Then drag it into any app, or click to copy it'];
        this._emptyTitle.textContent = title;
        this._emptyHint.textContent = hint;
    }

    /** Redraws from the shelf's list; a file's tile is made once and kept while it stays. */
    update() {
        const items = this.items;
        const wanted = new Set(items.map(item => item.uri));
        for (const [uri, tile] of this._tileFor) {
            if (!wanted.has(uri)) {
                tile.remove();
                this._tileFor.delete(uri);
                previews.delete(uri);
            }
        }
        const tiles = items.map(item => {
            let tile = this._tileFor.get(item.uri);
            if (!tile) {
                tile = this._tile(item);
                this._tileFor.set(item.uri, tile);
            }
            return tile;
        });
        const same = tiles.length === this._tiles.children.length && tiles.every((tile, i) => this._tiles.children[i] === tile);
        if (!same)
            this._tiles.replaceChildren(...tiles);
        this._tiles.hidden = items.length === 0;
        this._empty.hidden = items.length > 0;
        this._syncZone();
    }

    /** One file: its preview, × to take it off (↓ to save one the Shelf keeps), and its name. */
    _tile(item) {
        const preview = el('div', 'isle-shelf-preview', [icon('isle-file', 28, 'isle-shelf-generic')]);
        const copied = el('div', 'isle-shelf-copied', [icon('isle-check-symbolic', 18)]);
        const remove = iconButton('isle-close-symbolic', 'isle-shelf-remove',
            () => daemon.call('Shelf1', 'Remove', {uri: item.uri}), {size: 10, label: 'Remove from the shelf'});
        const parts = [preview, copied, remove];
        if (item.own) {
            const save = iconButton('isle-save-symbolic', 'isle-shelf-remove isle-shelf-save', () => this._save(item),
                {size: 10, label: 'Save to Downloads'});
            parts.push(save);
        }
        for (const button of parts.slice(2))
            button.addEventListener('pointerdown', event => event.stopPropagation());
        const frame = el('div', 'isle-shelf-frame', parts);
        const tile = el('div', 'isle-shelf-tile', [frame, el('div', 'isle-shelf-name', [item.name])]);
        tile.title = item.path ?? item.name;
        tile.tabIndex = 0;

        // A press that moves is a drag out of the island, which Windows takes over; one that doesn't is a click.
        tile.addEventListener('pointerdown', event => {
            if (event.button !== 0)
                return;
            const start = {x: event.clientX, y: event.clientY};
            const move = moved => {
                if (Math.hypot(moved.clientX - start.x, moved.clientY - start.y) < DRAG_PX)
                    return;
                done();
                invoke('shelf_drag', {uris: [item.uri]});
            };
            const up = () => {
                done();
                this._onClicked(item, copied);
            };
            const done = () => {
                window.removeEventListener('pointermove', move);
                window.removeEventListener('pointerup', up);
            };
            window.addEventListener('pointermove', move);
            window.addEventListener('pointerup', up);
        });
        tile.addEventListener('keydown', event => {
            if (event.key === 'Enter' || event.key === ' ')
                this._onClicked(item, copied);
        });

        previewOf(item.uri).then(shown => {
            if (!shown)
                return;
            if (shown.image) {
                preview.classList.add('thumbnail');
                // A document's page shows whole, as in Explorer, rather than cut to the square.
                preview.classList.toggle('whole', Boolean(shown.whole));
                preview.style.backgroundImage = `url("${shown.image}")`;
                preview.replaceChildren();
            } else if (shown.icon) {
                const image = el('img', 'isle-shelf-icon');
                image.src = shown.icon;
                preview.replaceChildren(image);
            }
        });
        return tile;
    }

    /** A click copies the file, to paste into any app; a second one soon after opens it. */
    _onClicked(item, copied) {
        const now = performance.now();
        const double = this._lastClick.uri === item.uri && now - this._lastClick.at < DOUBLE_CLICK_MS;
        this._lastClick = {uri: double ? null : item.uri, at: now};
        if (double) {
            invoke('shelf_open', {uri: item.uri});
            return;
        }
        invoke('shelf_copy', {uri: item.uri}).then(done => {
            if (!done)
                return;
            copied.classList.add('shown');
            clearTimeout(copied._timer);
            copied._timer = setTimeout(() => copied.classList.remove('shown'), COPIED_MS);
        });
    }

    async _save(item) {
        try {
            const name = await invoke('shelf_save', {uri: item.uri});
            this.onNote?.(`Saved to Downloads as ${name}`);
        } catch (err) {
            this.onNote?.(String(err));
        }
    }
}
