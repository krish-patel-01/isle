// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The Timer page (gnome-extension/src/pages/timer.js): a ring with the time left and the phase, the lengths
// of work and break as steppers (locked from Start until Reset), and Start.

import {timerColor} from '../compact.js';
import {daemon} from '../daemon.js';
import {Ring, el, formatTime, icon, iconButton, setIcon} from '../widgets.js';

const PHASES = {idle: 'Focus', focus: 'Focus', short_break: 'Break', long_break: 'Long break'};
const RING_SIZE = 104;
const DEFAULT_STATE = {phase: 'idle', running: false, remaining_ms: 25 * 60000, duration_ms: 25 * 60000, today_sessions: 0};
const STEPPERS = [
    {key: 'focus_minutes', label: 'Work', min: 1, max: 180},
    {key: 'short_break_minutes', label: 'Break', min: 1, max: 60},
];

export class TimerPage {
    constructor() {
        this.name = 'focus';
        this.node = el('div', 'isle-page isle-timer');
        this._time = el('div', 'isle-timer-time', ['25:00']);
        this._phase = el('div', 'isle-timer-phase', ['Focus']);
        this._ring = new Ring({size: RING_SIZE, lineWidth: 5, trackOpacity: 0.14, child: el('div', 'isle-ring-inside', [this._time, this._phase])});

        this._steppers = STEPPERS.map(spec => this._stepper(spec));
        const steppers = el('div', 'isle-steppers', this._steppers.map(s => s.row));

        this._reset = iconButton('isle-reset', 'isle-button-round', () => daemon.call('pomodoro', 'Stop'), {size: 16, label: 'Reset'});
        this._playIcon = icon('isle-play-symbolic', 13);
        this._playLabel = el('span', 'isle-start-pill-label', ['Start']);
        this._play = el('button', 'isle-start-pill', [this._playIcon, this._playLabel]);
        this._play.addEventListener('click', () => {
            const state = daemon.state.pomodoro ?? DEFAULT_STATE;
            daemon.call('pomodoro', state.running ? 'Pause' : 'Start');
        });
        const controls = el('div', 'isle-timer-controls', [this._reset, this._play]);

        const spacer = () => el('div', 'grow');
        this.node.append(this._ring.node, spacer(), steppers, spacer(), controls);
        daemon.connect(key => {
            if (key === 'pomodoro' || key === 'pomodoroConfig')
                this._update();
        });
        this._update();
    }

    get preferredWidth() {
        return null;
    }

    /** "Work  25  −  +": a pill whose length can be typed in or stepped. */
    _stepper({key, label, min, max}) {
        const entry = el('input', 'isle-stepper-entry');
        entry.type = 'text';
        entry.inputMode = 'numeric';
        entry.maxLength = 3;
        const set = minutes => {
            const config = {...(daemon.state.pomodoroConfig ?? {})};
            config[key] = Math.min(max, Math.max(min, Math.round(minutes)));
            daemon.call('pomodoro', 'SetConfig', config);
        };
        const value = () => daemon.state.pomodoroConfig?.[key] ?? 25;
        entry.addEventListener('focus', () => {
            window.isleKeyboard?.(true);
            entry.select();
        });
        entry.addEventListener('blur', () => {
            window.isleKeyboard?.(false);
            const typed = parseInt(entry.value, 10);
            if (Number.isFinite(typed) && typed !== value())
                set(typed);
            else
                entry.value = value();
        });
        entry.addEventListener('keydown', event => {
            if (event.key === 'Enter')
                entry.blur();
            else if (event.key === 'Escape') {
                entry.value = value();
                entry.blur();
            } else if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
                event.preventDefault();
                set(value() + (event.key === 'ArrowUp' ? 1 : -1));
            }
        });
        const minus = iconButton('isle-minus-symbolic', 'isle-button-flat isle-stepper-button', () => set(value() - 1), {size: 12, label: `Shorter ${label.toLowerCase()}`});
        const plus = iconButton('isle-plus-symbolic', 'isle-button-flat isle-stepper-button', () => set(value() + 1), {size: 12, label: `Longer ${label.toLowerCase()}`});
        const row = el('div', 'isle-stepper', [el('span', 'isle-stepper-name', [label]), entry, minus, plus]);
        return {row, entry, key, value};
    }

    _update() {
        const state = daemon.state.pomodoro ?? DEFAULT_STATE;
        const started = state.phase !== 'idle' && (state.running || state.started_at_ms);
        this._phase.textContent = PHASES[state.phase] ?? 'Focus';
        this._playLabel.textContent = state.running ? 'Pause' : started ? 'Resume' : 'Start';
        setIcon(this._playIcon, state.running ? 'isle-pause-symbolic' : 'isle-play-symbolic');
        this._reset.style.visibility = state.phase !== 'idle' ? 'visible' : 'hidden';
        for (const stepper of this._steppers) {
            stepper.row.classList.toggle('isle-stepper-locked', state.phase !== 'idle');
            if (document.activeElement !== stepper.entry)
                stepper.entry.value = stepper.value();
        }
        this._tick();
        clearInterval(this._ticker);
        if (state.running)
            this._ticker = setInterval(() => this._tick(), 250);
    }

    _tick() {
        let state = daemon.state.pomodoro ?? DEFAULT_STATE;
        if (state.phase === 'idle') {
            const minutes = daemon.state.pomodoroConfig?.focus_minutes ?? 25;
            state = {...state, remaining_ms: minutes * 60000, duration_ms: minutes * 60000};
        }
        const remaining = state.running ? Math.max(0, state.ends_at_ms - Date.now()) : state.remaining_ms;
        this._time.textContent = formatTime(remaining + 999);
        const progress = state.phase === 'idle' || !state.duration_ms ? 0 : 1 - remaining / state.duration_ms;
        this._ring.set(progress, timerColor(state.phase));
    }
}
