// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The Apps page (gnome-extension/src/pages/apps.js): time in each app today, or over the last seven or 30 days, most
// used first, with a heading, the period's total and a Today / 7 days / 30 days switch. The Rust side times the
// app in front (app_usage.rs); the page asks for the sums as it opens, and once a minute while open.

import {el, icon} from '../widgets.js';

const {invoke} = window.__TAURI__.core;

const PERIODS = [
    {name: 'today', label: 'Today', total: 'today'},
    {name: 'week', label: '7 days', total: 'in 7 days'},
    {name: 'month', label: '30 days', total: 'in 30 days'},
];
const REFRESH_MS = 60000;
const PERIOD_KEY = 'isle.apps.period';

/** A span of time as "1h 15m", "25m", "<1m" or "0m" (format.js `hoursMinutes`). */
export function hoursMinutes(ms) {
    if (ms < 60000)
        return ms > 0 ? '<1m' : '0m';
    const minutes = Math.round(ms / 60000);
    const hours = Math.floor(minutes / 60);
    if (!hours)
        return `${minutes}m`;
    return minutes % 60 ? `${hours}h ${minutes % 60}m` : `${hours}h`;
}

export class AppsPage {
    constructor() {
        this.name = 'apps';
        this.node = el('div', 'isle-page isle-apps');
        this._period = 'today';
        try {
            this._period = PERIODS.some(({name}) => name === localStorage.getItem(PERIOD_KEY))
                ? localStorage.getItem(PERIOD_KEY) : 'today';
        } catch {}
        this._stats = null;
        this._total = el('span', 'isle-apps-total');
        this._buttons = new Map();
        const toggle = el('div', 'isle-segmented isle-apps-toggle', PERIODS.map(({name, label}) => {
            const button = el('button', 'isle-segment', [label]);
            button.addEventListener('click', () => this._show(name));
            this._buttons.set(name, button);
            return button;
        }));
        const bar = el('div', 'isle-apps-bar', [el('span', 'isle-apps-heading', ['App Usage']), this._total, toggle]);
        this._list = el('div', 'isle-apps-list');
        this._scroll = el('div', 'isle-apps-scroll', [this._list]);
        this._empty = el('div', 'isle-empty', [icon('isle-apps', 24), el('div', 'isle-empty-title', ['No usage yet']),
            el('div', 'isle-empty-hint', ['Time in the app in front shows up here'])]);
        this.node.append(bar, el('div', 'isle-apps-area', [this._scroll, this._empty]));
        this._syncButtons();
        this._update();
    }

    get preferredWidth() {
        return null;
    }

    _show(period) {
        if (period === this._period)
            return;
        this._period = period;
        try {
            localStorage.setItem(PERIOD_KEY, period);
        } catch {}
        this._syncButtons();
        this._update();
    }

    _syncButtons() {
        for (const [name, button] of this._buttons)
            button.classList.toggle('checked', name === this._period);
    }

    /** The rows of the period shown, and its total. */
    _update() {
        const apps = this._stats?.[this._period] ?? [];
        const total = apps.reduce((sum, app) => sum + Math.max(0, app.ms), 0);
        const period = PERIODS.find(({name}) => name === this._period);
        this._total.textContent = total > 0 ? `${hoursMinutes(total)} ${period.total}` : '';
        this._empty.style.display = apps.length ? 'none' : '';
        this._scroll.style.display = apps.length ? '' : 'none';
        const most = Math.max(1, ...apps.map(app => app.ms));
        this._list.replaceChildren(...apps.map(app => this._row(app, most)));
    }

    /** One app: its icon, its name, a bar against the most used, and its time. */
    _row(app, most) {
        const glyph = app.icon ? el('img', 'isle-apps-icon') : icon('isle-apps', 18);
        if (app.icon)
            glyph.src = app.icon;
        // A sliver shows even for a minute beside hours.
        const fill = el('div', 'isle-apps-fill');
        fill.style.width = `max(4px, ${(Math.max(0, app.ms) / most) * 100}%)`;
        const row = el('div', 'isle-apps-row', [glyph, el('span', 'isle-apps-name', [app.name || app.id]),
            el('div', 'isle-apps-track', [fill]), el('span', 'isle-apps-time', [hoursMinutes(app.ms)])]);
        row.setAttribute('aria-label', app.id);
        return row;
    }

    async _refresh() {
        try {
            this._stats = await invoke('app_usage_stats');
        } catch {}
        this._update();
    }

    /** Asks for the sums as the page opens, then once a minute while it is open. */
    setActive(active) {
        clearInterval(this._ticker);
        if (!active)
            return;
        this._refresh();
        this._ticker = setInterval(() => this._refresh(), REFRESH_MS);
    }
}
