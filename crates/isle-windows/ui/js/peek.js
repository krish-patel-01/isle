// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// Alerts in the island (gnome-extension/src/peek.js and alerts.js): a round tinted badge with an icon, a title
// over a dimmer line, and a button when there is something to do. For now: the focus timer's phases.

import {el, icon} from './widgets.js';

/** Widest an alert gets, its padding all round, and its height: a 32 px badge in 12 px of room. */
const MAX_WIDTH = 380;
const PADDING = 12;
export const PEEK_HEIGHT = 32 + 2 * PADDING;
/** How long an alert with something to do stays (alerts.js REMINDER_DURATION). */
export const REMINDER_DURATION = 8000;

const TONES = {
    neutral: [255, 255, 255],
    good: [48, 209, 88],
    focus: [48, 209, 88],
    bad: [255, 69, 58],
    warn: [255, 159, 10],
};

/** The badge's gradient: the tone's colour, brighter at the top (white, fainter, for a neutral one). */
function badgeStyle(rgb, neutral) {
    const [top, bottom] = neutral ? [0.16, 0.08] : [0.34, 0.16];
    const base = neutral ? [255, 255, 255] : rgb;
    return `linear-gradient(to bottom, rgba(${base}, ${top}), rgba(${base}, ${bottom}))`;
}

/** A focus session or a break ran out (alerts.js `pomodoroFinished`). */
export function pomodoroFinished(finished, next, state, start) {
    const waiting = !state?.running;
    const minutes = state ? Math.round(state.duration_ms / 60000) : 0;
    if (finished === 'focus') {
        return {
            key: 'pomodoro',
            icon: 'isle-timer-symbolic',
            title: 'Focus session done',
            subtitle: waiting ? 'Take a break when you’re ready' : `Enjoy a ${minutes}-minute break`,
            tone: 'good',
            action: waiting ? {label: 'Start break', callback: start} : null,
            duration: REMINDER_DURATION,
            page: 'focus',
        };
    }
    const names = {focus: 'Focus', short_break: 'Break', long_break: 'Long break'};
    return {
        key: 'pomodoro',
        icon: 'isle-timer-symbolic',
        title: next === 'focus' ? 'Break is over' : names[next] ?? 'Timer',
        subtitle: waiting ? 'Ready for another focus session?' : `${minutes} minutes of focus`,
        tone: 'focus',
        action: waiting ? {label: 'Start focus', callback: start} : null,
        duration: REMINDER_DURATION,
        page: 'focus',
    };
}

export class PeekView {
    constructor() {
        this.node = el('div', 'isle-view isle-peek');
        this._badge = el('div', 'isle-peek-icon');
        this._icon = icon('isle-timer-symbolic', 16);
        this._badge.append(this._icon);
        this._title = el('div', 'isle-peek-title');
        this._subtitle = el('div', 'isle-peek-subtitle');
        this._texts = el('div', 'isle-peek-texts', [this._title, this._subtitle]);
        this._accessory = el('div', 'isle-peek-accessory');
        this._row = el('div', 'isle-peek-row', [this._badge, this._texts, this._accessory]);
        this.node.append(this._row);
        this.alert = null;
        /** Called when the alert is clicked (not its button), and when its button is pressed. */
        this.onOpen = null;
        this.onDone = null;
        this.node.addEventListener('click', () => this.alert && this.onOpen?.(this.alert));
    }

    get preferredWidth() {
        return Math.min(MAX_WIDTH, Math.ceil(this._row.scrollWidth));
    }

    get height() {
        return PEEK_HEIGHT;
    }

    show(alert) {
        const fresh = alert.key !== this.alert?.key || alert.title !== this.alert?.title;
        this.alert = alert;
        const rgb = TONES[alert.tone ?? 'neutral'] ?? TONES.neutral;
        const neutral = (alert.tone ?? 'neutral') === 'neutral';
        this._badge.style.background = badgeStyle(rgb, neutral);
        this._icon.style.color = `rgb(${rgb})`;
        this._icon.style.setProperty('--icon', `url("icons/${alert.icon}.svg")`);
        this._title.textContent = alert.title;
        this._subtitle.textContent = alert.subtitle ?? '';
        this._subtitle.style.display = alert.subtitle ? '' : 'none';
        this._accessory.replaceChildren();
        if (alert.action) {
            const button = el('button', 'isle-pill isle-pill-small', [alert.action.label]);
            button.addEventListener('click', event => {
                event.stopPropagation();
                alert.action.callback();
                this.onDone?.(alert);
            });
            this._accessory.append(button);
        }
        // Its icon moves once, as a new alert comes, once the island has made room.
        if (fresh) {
            this._badge.classList.remove('bounce');
            setTimeout(() => this._badge.classList.add('bounce'), 350);
        }
    }
}
