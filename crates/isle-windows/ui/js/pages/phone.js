// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The Shelf's Phone tab (gnome-extension/src/pages/phone.js), only while sharing: the QR code beside how to
// connect, then the phones asking (Allow or Deny, by the code each one's page shows) and those let in.

import {el, icon, iconButton} from '../widgets.js';

const {invoke} = window.__TAURI__.core;

/** The tallest the code may be (CSS px): the page's height less its top padding. */
const CODE_HEIGHT = 144;
/** White modules round the code, as scanners want. */
const QUIET = 2;
const CODE_RADIUS = 10;
const MAX_ASKS_SHOWN = 2;
const MAX_ROWS = 3;
const COPIED_MS = 1500;
/** A phone taken off or gone says so this long in its row, which then fades away. */
const LEAVING_MS = 3000;
const FADE_MS = 200;
/** What the island says of the network, by Isle's `network`. */
const NETWORK_NOTES = {
    public: 'This network looks public · use your phone’s hotspot',
    open: 'Open Wi-Fi: others nearby could see the files · try a hotspot',
    unknown: 'Only share on a network you trust',
};
const STEPS = ['Same Wi-Fi as this computer, or its hotspot', 'Scan the code with the camera', 'Allow it here with the code it shows'];

/** The QR code, black on white, each module whole device pixels so it is sharp at any scale. */
export class PhoneCode {
    constructor() {
        this.node = el('canvas', 'isle-phone-code');
        this.node.setAttribute('aria-label', 'Code to open the Shelf on your phone');
        this._rows = [];
    }

    set rows(rows) {
        const same = rows.length === this._rows.length && rows.every((row, i) => row === this._rows[i]);
        if (same)
            return;
        this._rows = rows;
        this._paint();
    }

    _paint() {
        const canvas = this.node;
        if (!this._rows.length) {
            canvas.width = canvas.height = 0;
            canvas.style.width = canvas.style.height = '0px';
            return;
        }
        const count = this._rows.length + 2 * QUIET;
        const ratio = window.devicePixelRatio || 1;
        const module = Math.max(1, Math.floor(CODE_HEIGHT * ratio / count));
        const size = module * count;
        canvas.width = canvas.height = size;
        canvas.style.width = canvas.style.height = `${size / ratio}px`;
        const cr = canvas.getContext('2d');
        cr.clearRect(0, 0, size, size);
        const radius = Math.min(CODE_RADIUS * ratio, QUIET * module);
        cr.fillStyle = '#ffffff';
        cr.beginPath();
        cr.roundRect(0, 0, size, size, radius);
        cr.fill();
        cr.fillStyle = '#000000';
        this._rows.forEach((row, y) => {
            for (let x = 0; x < row.length; x++) {
                if (row[x] === '1')
                    cr.fillRect((QUIET + x) * module, (QUIET + y) * module, module, module);
            }
        });
    }
}

/** How to connect, the phones asking and those let in. `onAnswer(id, allow)` and `onRemove(id)` act. */
export class PhoneView {
    constructor() {
        this.node = el('div', 'isle-phone');
        this.onAnswer = null;
        this.onRemove = null;
        this._title = el('div', 'isle-phone-title', ['Scan with your phone']);
        this._stepTexts = STEPS.map(text => el('div', 'isle-phone-step-text', [text]));
        this._steps = el('div', 'isle-phone-steps', this._stepTexts.map((words, i) =>
            el('div', 'isle-phone-step', [el('div', 'isle-phone-step-number', [String(i + 1)]), words])));
        this._asks = el('div', 'isle-phone-rows');
        this._phones = el('div', 'isle-phone-rows');
        this._noteText = el('div', 'isle-phone-note-text');
        this._note = el('div', 'isle-phone-note', [icon('isle-warning-symbolic', 12, 'isle-phone-note-icon'), this._noteText]);
        this._copyText = el('div', 'isle-phone-copy-text', ['Can’t scan? Send the link another way']);
        this._copy = el('button', 'isle-pill isle-pill-small', ['Copy link']);
        this._copy.addEventListener('click', () => this._copyNow());
        this._copyRow = el('div', 'isle-phone-copy', [this._copyText, this._copy]);
        this.node.append(this._title, this._steps, this._asks, this._phones, this._note, el('div', 'isle-grow'), this._copyRow);
        this._rows = new Map();
        this._removing = new Set();
        this._state = null;
        this._url = null;
        this._sync();
    }

    /** The session's address, for Copy link. */
    set url(url) {
        this._url = url;
        this._sync();
    }

    /** Shows the sharing state (Share1's State). */
    update(state) {
        this._state = state;
        const listed = state?.phones ?? [];
        for (const id of [...this._removing]) {
            if (!listed.some(phone => phone.id === id))
                this._removing.delete(id);
        }
        const phones = listed.filter(phone => !this._removing.has(phone.id));
        const asks = (state?.asking ?? []).slice(0, MAX_ASKS_SHOWN);
        this._syncRows(this._asks, asks, ask => this._askRow(ask));
        this._syncRows(this._phones, phones, phone => this._phoneRow(phone));
        let rows = this._asks.children.length + this._phones.children.length;
        for (const [key, row] of [...this._rows]) {
            if (rows <= MAX_ROWS)
                break;
            if (row._leaving) {
                this._drop(key, row);
                rows--;
            }
        }
        this._sync();
    }

    /** Sharing is off: every row goes at once, so a new start shows the steps. */
    reset() {
        this._removing.clear();
        for (const [key, row] of [...this._rows])
            this._drop(key, row);
        this.update(null);
    }

    _sync() {
        const state = this._state;
        const asks = this._asks.children.length;
        const phones = this._phones.children.length;
        const connected = asks + phones > 0;
        const note = NETWORK_NOTES[state?.network] ?? null;
        this._steps.hidden = connected;
        const warn = ['public', 'open'].includes(state?.network);
        this._stepTexts[0].textContent = warn ? note : STEPS[0];
        this._stepTexts[0].classList.toggle('isle-phone-warning', warn);
        this._title.hidden = !(asks === 0 && phones < 3);
        this._note.hidden = !(connected && note && asks + phones < 3);
        this._noteText.textContent = note ?? '';
        this._copyRow.hidden = connected || !this._url;
        this._asks.hidden = asks === 0;
        this._phones.hidden = phones === 0;
    }

    /** One row per item, kept while its phone stays; one whose phone went says so first. */
    _syncRows(box, items, make) {
        const asks = box === this._asks;
        const keyOf = item => asks ? `ask:${item.id}:${item.code}` : `in:${item.id}`;
        const keys = items.map(keyOf);
        for (const [key, row] of [...this._rows]) {
            if (row.parentNode !== box)
                continue;
            if (keys.includes(key)) {
                if (row._leaving)
                    this._drop(key, row);
            } else if (!row._leaving) {
                if (asks)
                    this._drop(key, row);
                else
                    this._leave(row, `${row._device} left`);
            }
        }
        items.forEach((item, index) => {
            let row = this._rows.get(keys[index]);
            if (!row) {
                row = make(item);
                this._rows.set(keys[index], row);
                box.insertBefore(row, box.children[index] ?? null);
            } else if (!asks) {
                this._refresh(row, item);
            }
        });
    }

    _drop(key, row) {
        clearTimeout(row._timer);
        this._rows.delete(key);
        row.remove();
    }

    /** A phone asking: its code, what it is and where from; Deny and Allow answer this phone. */
    _askRow({id, code, device, address}) {
        const words = el('div', 'isle-phone-ask-words', [el('div', 'isle-phone-code-text', [`Code ${code}`]),
            el('div', 'isle-phone-hint', [`${device} · ${address}`])]);
        const deny = el('button', 'isle-pill isle-pill-small', ['Deny']);
        const allow = el('button', 'isle-pill isle-pill-small isle-pill-white', ['Allow']);
        deny.setAttribute('aria-label', `Deny ${device} with code ${code}`);
        allow.setAttribute('aria-label', `Allow ${device} with code ${code}`);
        deny.addEventListener('click', () => this.onAnswer?.(id, false));
        allow.addEventListener('click', () => this.onAnswer?.(id, true));
        return el('div', 'isle-phone-row isle-phone-ask', [words, deny, allow]);
    }

    /** A phone let in, with × to take it off. */
    _phoneRow(phone) {
        const {id, device} = phone;
        const row = el('div', 'isle-phone-row isle-phone-status');
        row._dot = el('div', 'isle-phone-dot');
        row._text = el('div', 'isle-phone-status-text');
        row._remove = iconButton('isle-close-symbolic', 'isle-phone-remove', () => {
            this._removing.add(id);
            this.onRemove?.(id);
            this._leave(row, `✓ ${row._device} disconnected`);
        }, {size: 10, label: `Take ${device} off`});
        row.append(row._dot, row._text, row._remove);
        this._refresh(row, phone);
        return row;
    }

    /** Here (a green dot), or away (dim, "away"; still in). */
    _refresh(row, {device, address, here = true}) {
        if (row._leaving)
            return;
        row._device = device;
        row._text.textContent = here ? `${device} · ${address}` : `${device} · ${address} · away`;
        row._dot.classList.toggle('on', here);
        row.classList.toggle('isle-phone-away', !here);
    }

    _leave(row, text) {
        if (row._leaving)
            return;
        row._leaving = true;
        row._text.textContent = text;
        row.classList.add('isle-phone-leaving');
        row._remove.style.visibility = 'hidden';
        row._timer = setTimeout(() => {
            row.classList.add('gone');
            row._timer = setTimeout(() => {
                for (const [key, kept] of this._rows) {
                    if (kept === row)
                        this._rows.delete(key);
                }
                row.remove();
                this._sync();
            }, FADE_MS);
        }, LEAVING_MS);
    }

    /** Copies the link; the button says so a moment. */
    async _copyNow() {
        if (!this._url)
            return;
        await invoke('copy_text', {text: this._url});
        this._copy.textContent = 'Copied';
        clearTimeout(this._copiedTimer);
        this._copiedTimer = setTimeout(() => (this._copy.textContent = 'Copy link'), COPIED_MS);
    }
}
