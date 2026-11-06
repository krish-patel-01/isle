// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// Voice in the island: while dictating, a red microphone badge beside the microphone's live wave; then the wave
// shimmers while the words are worked out, and the island says what it typed. The assistant listens the same way.

import {el, icon, setIcon} from './widgets.js';

/** As tall as an alert's row is wide enough for: a 28 px badge in 8 px of room. */
const HEIGHT = 44;
const PADDING = 8;
/** The wave: this many bars, each this wide with this gap, at most this tall. */
const BARS = 26;
const BAR = 3;
const GAP = 2;
const WAVE_HEIGHT = 24;
const WAVE_WIDTH = BARS * (BAR + GAP) - GAP;
/** How fast a bar rises to the level, and falls back (per second). */
const RISE = 28;
const FALL = 9;

const PHASES = {
    listening: {icon: 'isle-mic-symbolic', rgb: [255, 69, 58]},
    working: {icon: 'isle-mic-symbolic', rgb: [255, 159, 10]},
    done: {icon: 'isle-check-symbolic', rgb: [48, 209, 88]},
    error: {icon: 'isle-warning-symbolic', rgb: [255, 255, 255]},
};

/** A level (the microphone's root mean square, speech mostly 0.02 to 0.2) as a bar's share of the wave's height. */
function shareOf(level) {
    return Math.min(1, Math.sqrt(Math.max(0, level)) * 2.4);
}

export class VoiceView {
    constructor() {
        this._icon = icon('isle-mic-symbolic', 14);
        this._badge = el('div', 'isle-voice-badge', [this._icon]);
        this._canvas = el('canvas', 'isle-voice-wave');
        this._label = el('div', 'isle-voice-label');
        this._row = el('div', 'isle-voice-row', [this._badge, this._canvas, this._label]);
        this.node = el('div', 'isle-view isle-voice', [this._row]);
        this.state = null;
        /** Each bar's share now, oldest on the left; and where the newest is heading. */
        this._bars = new Array(BARS).fill(0);
        this._target = 0;
        this._frame = 0;
    }

    get preferredWidth() {
        return Math.ceil(this._row.scrollWidth);
    }

    get height() {
        return HEIGHT;
    }

    /** `state`: {mode, phase, text, handsFree}, or null for nothing. */
    set(state) {
        this.state = state;
        if (!state) {
            this._stop();
            return;
        }
        const phase = PHASES[state.phase] ?? PHASES.error;
        const [r, g, b] = phase.rgb;
        const neutral = state.phase === 'error';
        this._badge.style.background = neutral
            ? 'linear-gradient(to bottom, rgba(255,255,255,0.16), rgba(255,255,255,0.08))'
            : `linear-gradient(to bottom, rgba(${r},${g},${b},0.34), rgba(${r},${g},${b},0.16))`;
        this._icon.style.color = `rgb(${r},${g},${b})`;
        setIcon(this._icon, phase.icon);
        this._badge.classList.toggle('pulse', state.phase === 'listening');
        const waving = state.phase === 'listening' || state.phase === 'working';
        this._canvas.hidden = !waving;
        this._label.textContent = waving ? state.text ?? '' : state.text || '';
        this._label.classList.toggle('dim', waving);
        this._row.classList.toggle('message', !waving);
        this._color = `${r},${g},${b}`;
        if (state.phase === 'listening' && this._phase !== 'listening')
            this._bars.fill(0);
        this._phase = state.phase;
        if (waving)
            this._start();
        else
            this._stop();
    }

    /** The microphone's level, about 30 times a second while listening. */
    pushLevel(level) {
        this._target = shareOf(level);
    }

    _start() {
        if (this._frame)
            return;
        const ratio = window.devicePixelRatio || 1;
        this._canvas.width = WAVE_WIDTH * ratio;
        this._canvas.height = WAVE_HEIGHT * ratio;
        this._canvas.style.width = `${WAVE_WIDTH}px`;
        this._canvas.style.height = `${WAVE_HEIGHT}px`;
        this._last = performance.now();
        this._shift = 0;
        const step = now => {
            this._frame = requestAnimationFrame(step);
            const dt = Math.min(0.05, (now - this._last) / 1000);
            this._last = now;
            this._draw(now / 1000, dt);
        };
        this._frame = requestAnimationFrame(step);
    }

    _stop() {
        cancelAnimationFrame(this._frame);
        this._frame = 0;
    }

    _draw(t, dt) {
        const bars = this._bars;
        if (this._phase === 'listening') {
            // The newest bar follows the level; the wave scrolls left a bar every 45 ms.
            const newest = bars.length - 1;
            const rate = this._target > bars[newest] ? RISE : FALL;
            bars[newest] += (this._target - bars[newest]) * (1 - Math.exp(-dt * rate));
            this._shift += dt;
            while (this._shift >= 0.045) {
                this._shift -= 0.045;
                bars.push(bars.shift());
                bars[newest] = bars[newest - 1];
            }
        } else {
            // Working: a soft wave runs along the bars.
            for (let i = 0; i < bars.length; i++) {
                const wave = 0.18 + 0.22 * (0.5 + 0.5 * Math.sin(t * 7 - i * 0.55));
                bars[i] += (wave - bars[i]) * (1 - Math.exp(-dt * 10));
            }
        }
        const ratio = window.devicePixelRatio || 1;
        const context = this._canvas.getContext('2d');
        context.setTransform(ratio, 0, 0, ratio, 0, 0);
        context.clearRect(0, 0, WAVE_WIDTH, WAVE_HEIGHT);
        for (let i = 0; i < bars.length; i++) {
            const height = Math.max(BAR, bars[i] * WAVE_HEIGHT);
            const x = i * (BAR + GAP);
            const y = (WAVE_HEIGHT - height) / 2;
            // Older bars fade a little, so the wave reads as moving left.
            const alpha = 0.45 + 0.55 * (i / (bars.length - 1));
            context.fillStyle = this._phase === 'listening'
                ? `rgba(255,255,255,${alpha})` : `rgba(${this._color},${0.5 + 0.5 * alpha})`;
            context.beginPath();
            context.roundRect(x, y, BAR, height, BAR / 2);
            context.fill();
        }
    }
}

export const VOICE_PADDING = PADDING;
