// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// Usage (gnome-extension/src/pages/usage.js): how much of each AI plan's limits is used (Claude, ChatGPT as Codex
// sees it), a bar per limit. One provider at a time, scrolled through like a widget stack, with dots to switch.

import {daemon} from '../daemon.js';
import {el, icon} from '../widgets.js';

/** A limit warns and then turns red at these shares (Settings: usage-warn, usage-critical). */
const marks = () => ({
    warn: daemon.state.settings?.['usage-warn'] ?? 80,
    critical: daemon.state.settings?.['usage-critical'] ?? 95,
});
const ACCENTS = {claude: [217, 120, 87], codex: [15, 163, 128], grok: [237, 240, 245]};
const FALLBACK = [10, 132, 255];
const LOGOS = {claude: 'isle-claude', codex: 'isle-openai', grok: 'isle-grok'};
const WHEEL_STEP = 100;
const WHEEL_REPEAT_MS = 200;
const SLIDE_MS = 220;

const rgb = ([r, g, b], alpha = 1) => `rgba(${r}, ${g}, ${b}, ${alpha})`;

/** "resets in 1h 20m", "resets in 2d 5h", "resets in 12m". */
function resets(ms) {
    const minutes = Math.max(0, Math.round((ms - Date.now()) / 60000));
    if (minutes < 1)
        return 'resets soon';
    const days = Math.floor(minutes / 1440);
    const hours = Math.floor((minutes % 1440) / 60);
    if (days > 0)
        return hours ? `resets in ${days}d ${hours}h` : `resets in ${days}d`;
    if (hours > 0)
        return minutes % 60 ? `resets in ${hours}h ${minutes % 60}m` : `resets in ${hours}h`;
    return `resets in ${minutes}m`;
}

/** "Updated 14:32", or "From Codex, 14:32"; on another day, with its name. */
function updated(ms, fromCodex) {
    if (!ms)
        return '';
    const at = new Date(ms);
    const today = at.toDateString() === new Date().toDateString();
    const time = at.toLocaleTimeString([], {hour: 'numeric', minute: '2-digit'});
    const when = today ? time : `${at.toLocaleDateString('en-GB', {weekday: 'short'})} ${time}`;
    return fromCodex ? `From Codex, ${when}` : `Updated ${when}`;
}

export class UsagePage {
    constructor() {
        this.name = 'usage';
        this.node = el('div', 'isle-page isle-usage');
        this._stage = el('div', 'isle-usage-stage');
        this._dots = el('div', 'isle-usage-dots');
        this.node.append(this._stage, this._dots);
        this._index = 0;
        this._lastFlip = 0;
        this._wheel = 0;
        this.node.addEventListener('wheel', event => {
            event.preventDefault();
            this._wheel += event.deltaY;
            const now = performance.now();
            if (Math.abs(this._wheel) >= WHEEL_STEP && now - this._lastFlip >= WHEEL_REPEAT_MS) {
                this._show(this._index + Math.sign(this._wheel), Math.sign(this._wheel));
                this._wheel = 0;
                this._lastFlip = now;
            }
        }, {passive: false});
        daemon.connect(() => this._build(), 'usage');
        daemon.connect(() => this._build(), 'settings');
        this._build();
    }

    get preferredWidth() {
        return null;
    }

    get _providers() {
        return daemon.state.usage?.providers ?? [];
    }

    setActive(active) {
        clearInterval(this._ticker);
        if (!active)
            return;
        daemon.call('Usage1', 'Refresh');
        this._build();
        // "resets in 12m" counts down while it is on screen.
        this._ticker = setInterval(() => this._build(), 30000);
    }

    _build() {
        const providers = this._providers;
        this._index = Math.min(this._index, Math.max(0, providers.length - 1));
        if (providers.length === 0) {
            this._stage.replaceChildren(el('div', 'col isle-empty', [
                icon('isle-gauge', 28, 'isle-empty-icon'),
                el('div', 'isle-empty-title', ['No AI plans here']),
                el('div', 'isle-empty-hint', ['Sign in to Claude Code or Codex to see their limits']),
            ]));
            this._dots.replaceChildren();
            return;
        }
        this._stage.replaceChildren(this._content(providers[this._index]));
        this._dots.replaceChildren(...(providers.length > 1 ? providers.map((provider, i) => {
            const dot = el('button', `isle-usage-dot${i === this._index ? ' shown' : ''}`);
            dot.title = provider.name;
            dot.addEventListener('click', () => this._show(i, Math.sign(i - this._index)));
            return dot;
        }) : []));
    }

    _show(index, direction) {
        const count = this._providers.length;
        if (count < 2)
            return;
        index = (index + count) % count;
        if (index === this._index)
            return;
        this._index = index;
        this._build();
        const content = this._stage.firstChild;
        content.style.transition = 'none';
        content.style.transform = `translateY(${direction * 24}px)`;
        content.style.opacity = '0';
        requestAnimationFrame(() => requestAnimationFrame(() => {
            content.style.transition = `transform ${SLIDE_MS}ms cubic-bezier(0.25, 1, 0.5, 1), opacity ${SLIDE_MS}ms`;
            content.style.transform = '';
            content.style.opacity = '1';
        }));
    }

    /** One provider: logo, name, plan and freshness, then up to three limits. */
    _content(provider) {
        const accent = ACCENTS[provider.id] ?? FALLBACK;
        const head = el('div', 'isle-usage-head');
        if (LOGOS[provider.id]) {
            head.append(icon(LOGOS[provider.id], 18, 'isle-usage-logo'));
        } else {
            const dot = el('div', 'isle-usage-mark');
            dot.style.backgroundColor = rgb(accent);
            head.append(dot);
        }
        head.append(el('span', 'isle-usage-name', [provider.name]));
        if (provider.plan)
            head.append(el('span', 'isle-usage-plan', [provider.plan]));
        const fromCodex = provider.note?.startsWith('As of');
        const warning = provider.note && !fromCodex;
        head.append(el('span', `isle-usage-stamp${warning ? ' warning' : ''}`,
            [warning ? provider.note : updated(provider.updated_ms, fromCodex)]));

        const limits = el('div', 'isle-usage-limits');
        for (const window of provider.windows.slice(0, 3))
            limits.append(this._limit(window, accent));
        if (provider.windows.length === 0)
            limits.append(el('div', 'isle-usage-empty', [provider.empty || 'No limits reported yet']));
        return el('div', 'isle-usage-content', [head, limits]);
    }

    /** A limit: its name, when it resets and its share; the bar below, in the plan's colour until nearly used up. */
    _limit({label, percent, resets_at_ms: resetsAt}, accent) {
        const used = Math.max(0, Math.min(100, percent));
        const shown = Math.round(used);
        const level = shown >= marks().critical ? 'critical' : shown >= marks().warn ? 'warn' : 'normal';
        const line = el('div', 'isle-usage-line', [el('span', 'isle-usage-window', [label])]);
        if (resetsAt > 0)
            line.append(el('span', 'isle-usage-reset', [resets(resetsAt)]));
        line.append(el('span', `isle-usage-percent ${level}`, [`${shown}%`]));
        const fill = el('div', `isle-usage-fill ${level}`);
        fill.style.width = `${used}%`;
        if (level !== 'critical')
            fill.style.backgroundColor = rgb(accent);
        return el('div', 'isle-usage-limit', [line, el('div', 'isle-usage-track', [fill])]);
    }
}
