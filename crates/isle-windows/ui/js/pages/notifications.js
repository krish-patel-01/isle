// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The Notifications page (gnome-extension/src/pages/notifications.js): a heading with Do Not Disturb and Clear (and,
// while it is on, what it does), over a card per notification, newest first. A click opens the app it came from.

import {daemon} from '../daemon.js';
import {el, icon, iconButton, setButtonIcon} from '../widgets.js';

/** "now", "5m", "2h", "Tue", "12 Oct": how long ago (format.js `ago`). */
export function ago(ms) {
    const minutes = Math.floor((Date.now() - ms) / 60000);
    if (minutes < 1)
        return 'now';
    if (minutes < 60)
        return `${minutes}m`;
    if (minutes < 24 * 60)
        return `${Math.floor(minutes / 60)}h`;
    const date = new Date(ms);
    return minutes < 6 * 24 * 60 ? date.toLocaleDateString('en-GB', {weekday: 'short'})
        : date.toLocaleDateString('en-GB', {day: 'numeric', month: 'short'});
}

/** The app's icon beside its card: its own picture, or a bell on a grey rounded square. */
export function notificationBadge(note, size = 20) {
    if (note.icon) {
        const image = el('img', 'isle-notification-icon');
        image.src = note.icon;
        image.width = image.height = size;
        return image;
    }
    return el('div', 'isle-notification-glyph', [icon('isle-bell', 12)]);
}

export class NotificationsPage {
    constructor() {
        this.name = 'notifications';
        this.node = el('div', 'isle-page isle-notifications');
        this._dndButton = iconButton('isle-bell', 'isle-button-round isle-button-small isle-dnd-toggle',
            () => daemon.call('Notifications1', 'SetDnd', !this._state.dnd), {size: 13, label: 'Do Not Disturb'});
        this._clearButton = el('button', 'isle-pill isle-pill-small isle-notifications-clear', ['Clear']);
        this._clearButton.addEventListener('click', () => daemon.call('Notifications1', 'ClearAll'));
        const bar = el('div', 'isle-notifications-bar', [el('span', 'isle-notifications-heading', ['Notifications']),
            this._dndButton, this._clearButton]);
        this._dndNote = el('div', 'isle-notifications-dnd-note', [icon('isle-moon', 10),
            el('span', 'isle-notifications-dnd-text', ['Do Not Disturb: new ones wait here, without popping up'])]);
        this._list = el('div', 'isle-notification-list');
        this._scroll = el('div', 'isle-notification-scroll', [this._list]);
        this._empty = el('div', 'isle-empty', [icon('isle-bell', 24), el('div', 'isle-empty-title', ['No notifications']),
            el('div', 'isle-empty-hint', ['You’re all caught up'])]);
        this.node.append(bar, this._dndNote, el('div', 'isle-notification-area', [this._scroll, this._empty]));
        /** Called when the dot on the tab may change (unseen ones came or were seen). */
        this.onNews = null;
        daemon.connect(() => this._update(), 'notifications');
        this._update();
    }

    get preferredWidth() {
        return null;
    }

    get _state() {
        return daemon.state.notifications ?? {dnd: false, items: []};
    }

    /** Unseen ones put a dot on the tab. */
    get hasNews() {
        return this._state.items.some(note => note.unseen);
    }

    _update() {
        const {dnd, items} = this._state;
        this._dndButton.classList.toggle('checked', dnd);
        setButtonIcon(this._dndButton, dnd ? 'isle-moon' : 'isle-bell');
        this._dndNote.style.display = dnd ? '' : 'none';
        this._clearButton.style.display = items.length ? '' : 'none';
        this._list.replaceChildren(...items.map(note => this._row(note)));
        this._empty.style.display = items.length ? 'none' : '';
        this._scroll.style.display = items.length ? '' : 'none';
        // Seen while open: no unread dot for something already on screen.
        if (this._active && this.hasNews)
            daemon.call('Notifications1', 'AcknowledgeAll');
        this.onNews?.();
    }

    /** A card: the app's icon, who, what on up to three lines, and when; × in its place under the pointer. */
    _row(note) {
        const texts = el('div', 'isle-notification-texts', [el('div', 'isle-notification-title', [note.title.replace(/\s+/g, ' ')])]);
        const body = note.body.replace(/\s+/g, ' ').trim();
        if (body)
            texts.append(el('div', 'isle-notification-body', [body]));
        const time = el('span', 'isle-notification-time', [ago(note.time_ms)]);
        const dismiss = iconButton('isle-close-symbolic', 'isle-button-flat isle-notification-dismiss',
            () => daemon.call('Notifications1', 'Dismiss', {id: note.id}), {size: 10, label: 'Dismiss'});
        const corner = el('div', 'isle-notification-corner', [time, dismiss]);
        const row = el('button', `isle-notification-row${body ? ' tall' : ''}`, [
            el('div', 'isle-notification', [notificationBadge(note), texts, corner])]);
        row.setAttribute('aria-label', note.app);
        row.addEventListener('click', () => {
            daemon.call('Notifications1', 'Open', {id: note.id});
            this.onClose?.();
        });
        return row;
    }

    setActive(active) {
        this._active = active;
        if (active) {
            if (this.hasNews)
                daemon.call('Notifications1', 'AcknowledgeAll');
            this._update();
        }
    }
}
