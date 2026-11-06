// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// Light on the island's edge, on a layer of its own over the shape and under the views.
//  - The aura (gnome-extension/src/glow.js `paintRim`): light rising from the edge, brightest along the bottom and
//    gone by the top of the sides, its colours flowing round. It swells with the voice, its streaks shine brighter
//    while the assistant works, and it beats like a heart while an alert waits to be seen or answered.
//  - A comet: a bright head with a fading tail that runs once down one side, along the bottom and up the other, in
//    an alert's colour, as the alert arrives.
// Drawn only while something is lit; nothing runs while the island is plain.

/** Colour stops across the island's width. */
const STOPS = 24;
/** How fast the colours flow round (widths per second). */
const FLOW_SPEED = 0.12;
/** Where the rising mask bends, and how strong it is there. */
const RISE_KNEE = 0.55, RISE_AT_KNEE = 0.35;
/** How fast the aura comes and goes (per second). */
const FADE_IN = 5, FADE_OUT = 2.5;
/** The voice's level: quick to rise with a syllable, slow to fall. */
const ATTACK = 18, RELEASE = 5, LEVEL_RATE = 6;
/** A heartbeat (an alert waiting to be seen or answered): a beat this long, the second ("dub") this far after the
 * first and this strong, beating this long before it settles to a steady glow (and a second to settle). */
const BEAT_S = 1, DUB_AT = 0.26, DUB = 0.6, HEART_S = 15, SETTLE_S = 1;
/** A breath while the microphone listens: this long and this deep, on and on. */
const BREATH_S = 2.4, BREATH_DEPTH = 0.45;
/** How dim the light is between beats, and where it settles. */
const BEAT_REST = 0.35, BEAT_SETTLED = 0.7;

/** 0..1, the heartbeat `t` seconds in: a sharp beat and a softer one, then rest. */
function heartbeat(t) {
    const phase = (t % BEAT_S) / BEAT_S;
    const spike = x => Math.exp(-((x / 0.055) ** 2));
    return Math.max(spike(phase), DUB * spike(phase - DUB_AT), spike(phase - 1));
}
/** A comet's run, in seconds, and its tail as a share of the edge's length. */
const COMET_S = 1.25, TAIL = 0.32;

/** The red light, in 0..255. */
export const RED = [255, 36, 24];

/** Isle's red, the assistant's colours (Isle's classic glow), and dictation's, per phase. */
export const PALETTES = {
    // Ultron's red: a hot red, a lighter one and a deep crimson.
    red: [[1, 0.1, 0.06], [1, 0.32, 0.24], [0.72, 0.02, 0.06]],
    // The voice's: a bright green, a lighter one and a deep emerald.
    voice: [[0.12, 0.95, 0.38], [0.5, 1, 0.6], [0.02, 0.62, 0.3]],
    assistant: [[0.56, 0.4, 1], [0.27, 0.56, 1], [0.22, 0.84, 0.96]],
    listening: [[1, 0.27, 0.23], [1, 0.4, 0.55], [1, 0.52, 0.22]],
    working: [[1, 0.62, 0.04], [1, 0.42, 0.25], [1, 0.8, 0.3]],
    done: [[0.19, 0.82, 0.35], [0.35, 0.88, 0.75], [0.7, 0.94, 0.35]],
};

/** Three colours around `rgb` (0..255): it, a lighter one, and one turned a little round the wheel. */
export function paletteAround([r, g, b]) {
    const c = [r / 255, g / 255, b / 255];
    if (c.every(v => v > 0.9))
        return [[1, 1, 1], [0.78, 0.85, 1], [0.92, 0.92, 0.98]];
    const lighter = c.map(v => v + (1 - v) * 0.4);
    const turned = [c[1] * 0.3 + c[0] * 0.7, c[2] * 0.3 + c[1] * 0.7, c[0] * 0.3 + c[2] * 0.7];
    return [c, lighter, turned];
}

const bump = (x, at, width) => {
    const d = Math.abs(((x - at) % 1 + 1.5) % 1 - 0.5);
    return Math.exp(-(d * d) / (2 * width * width));
};

/** Where the two streaks are at `time` (s), as shares of the width: one going right, one left. */
const streakAt = time => [(time * 0.11) % 1, 1 - (time * 0.075 + 0.4) % 1];

const rgba = (c, a) => `rgba(${Math.round(c[0] * 255)}, ${Math.round(c[1] * 255)}, ${Math.round(c[2] * 255)}, ${a})`;

/** The light's path: from the top of the left side, down, along the bottom and up the right (the shape's `{cx, w, h, r}`). */
function traceSides(cr, {cx, w, h, r}) {
    const left = cx - w / 2, right = cx + w / 2;
    cr.beginPath();
    cr.moveTo(left, 0);
    cr.lineTo(left, h - r);
    if (r > 0)
        cr.arc(left + r, h - r, r, Math.PI, Math.PI / 2, true);
    cr.lineTo(right - r, h);
    if (r > 0)
        cr.arc(right - r, h - r, r, Math.PI / 2, 0, true);
    cr.lineTo(right, 0);
}

/** The same path as points about 2 px apart, each with how far along it is. */
function sidePoints({cx, w, h, r}) {
    const left = cx - w / 2, right = cx + w / 2;
    const points = [];
    const line = (x0, y0, x1, y1) => {
        const n = Math.max(1, Math.ceil(Math.hypot(x1 - x0, y1 - y0) / 2));
        for (let i = 0; i < n; i++)
            points.push([x0 + (x1 - x0) * i / n, y0 + (y1 - y0) * i / n]);
    };
    const arc = (x, y, from, to) => {
        const n = Math.max(1, Math.ceil(Math.abs(to - from) * r / 2));
        for (let i = 0; i < n; i++) {
            const a = from + (to - from) * i / n;
            points.push([x + r * Math.cos(a), y + r * Math.sin(a)]);
        }
    };
    line(left, 0, left, h - r);
    if (r > 0)
        arc(left + r, h - r, Math.PI, Math.PI / 2);
    line(left + r, h, right - r, h);
    if (r > 0)
        arc(right - r, h - r, Math.PI / 2, 0);
    line(right, h - r, right, 0);
    points.push([right, 0]);
    let along = 0;
    return points.map((p, i) => {
        if (i)
            along += Math.hypot(p[0] - points[i - 1][0], p[1] - points[i - 1][1]);
        return [p[0], p[1], along];
    });
}

export class Glow {
    /** Draws on `canvas` (the window's size) round `island`'s shape. */
    constructor(canvas, island) {
        this._canvas = canvas;
        this._island = island;
        this._light = document.createElement('canvas');
        this._colors = null;
        this._strength = 0;
        this._mic = 0;
        this._micTarget = 0;
        this._level = 0;
        this._levelTarget = 0;
        this._work = 0;
        this._working = false;
        this._boost = 0;
        this._flow = 0;
        this._heart = null;
        this._comets = [];
        this._frame = 0;
        this._last = 0;
        this._reduced = window.matchMedia?.('(prefers-reduced-motion: reduce)');
        /** Animates even with motion reduced (Settings: Always animate). */
        this.alwaysMove = true;
    }

    /** Whether to keep the light still, as Windows asks with its Animation effects off (unless `alwaysMove`). */
    get _still() {
        return !this.alwaysMove && Boolean(this._reduced?.matches);
    }

    /** Lights the aura in `colors` (a list of 0..1 rgb; null fades it). `level` 0..1 is how bright it rests,
     * `working` brightens its streaks, `heartbeat` makes it beat for a while (an alert waiting), `breathing` makes it
     * breathe for as long as it is lit (the microphone listening). */
    aura(colors, {level = 0.3, working = false, heartbeat = false, breathing = false} = {}) {
        if (breathing && !this._breath)
            this._breath = {from: performance.now() / 1000};
        else if (!breathing)
            this._breath = null;
        if (colors)
            this._colors = colors;
        this._lit = Boolean(colors);
        this._levelTarget = level;
        this._working = working;
        if (heartbeat && !this._heart)
            this._heart = {from: performance.now() / 1000};
        else if (!heartbeat)
            this._heart = null;
        this._run();
    }

    /** The microphone's level (its root mean square), which swells the aura. */
    voice(level) {
        // Speech is around 0.01 to 0.1: a word lifts it well up, a shout to the top.
        this._micTarget = Math.min(1, Math.sqrt(Math.max(0, level)) * 4.5);
        if (this._lit)
            this._run();
    }

    /** The aura a moment brighter (something done). */
    pulse() {
        this._boost = 1;
        this._run();
    }

    /** A comet in `rgb` (0..255) runs once round the edge; with motion reduced (Windows' animation effects off),
     * the whole edge glows once instead, without moving. */
    comet(rgb) {
        // A new one replaces any still running: one light at a time reads best.
        this._comets = [{rgb: rgb.map(v => v / 255), from: performance.now() / 1000, still: this._still}];
        this._run();
    }

    /** Puts everything out at once. */
    clear() {
        this._lit = false;
        this._strength = 0;
        this._comets = [];
        this._heart = null;
        this._draw();
    }

    _run() {
        if (this._frame)
            return;
        this._last = performance.now() / 1000;
        const tick = () => {
            this._frame = 0;
            const now = performance.now() / 1000;
            const dt = Math.min(0.1, Math.max(0, now - this._last));
            this._last = now;
            this._step(now, dt);
            this._draw(now);
            if (this._strength > 0.002 || this._lit || this._comets.length)
                this._frame = requestAnimationFrame(tick);
        };
        this._frame = requestAnimationFrame(tick);
    }

    _step(now, dt) {
        const target = this._lit ? 1 : 0;
        const rate = target > this._strength ? FADE_IN : FADE_OUT;
        this._strength += (target - this._strength) * (1 - Math.exp(-dt * rate));
        if (!this._lit && this._strength < 0.002)
            this._strength = 0;
        this._micTarget *= Math.exp(-dt * 4);
        const attack = this._micTarget > this._mic ? ATTACK : RELEASE;
        this._mic += (this._micTarget - this._mic) * (1 - Math.exp(-dt * attack));
        this._boost *= Math.exp(-dt * 2.5);
        const level = this._levelTarget + 1.1 * this._mic + 0.55 * this._boost;
        this._level += (level - this._level) * (1 - Math.exp(-dt * LEVEL_RATE));
        this._work += ((this._working ? 1 : 0) - this._work) * (1 - Math.exp(-dt * 3));
        // A little faster while it works; summed up, so a change of speed never makes it jump.
        this._flow += this._still ? 0 : dt * (this._working ? 1.6 : 1);
        this._comets = this._comets.filter(comet => now - comet.from < COMET_S);
    }

    _draw(now = performance.now() / 1000) {
        const ratio = window.devicePixelRatio || 1;
        const width = Math.round(window.innerWidth * ratio), height = Math.round(window.innerHeight * ratio);
        // A window not laid out yet: nothing to draw on.
        if (!width || !height)
            return;
        for (const canvas of [this._canvas, this._light]) {
            if (canvas.width !== width || canvas.height !== height) {
                canvas.width = width;
                canvas.height = height;
            }
        }
        const cr = this._canvas.getContext('2d');
        cr.setTransform(1, 0, 0, 1, 0, 0);
        cr.clearRect(0, 0, width, height);
        const shape = this._island.shape;
        if (shape.w < 4 || shape.h < 4)
            return;
        cr.setTransform(ratio, 0, 0, ratio, 0, 0);
        if (this._strength > 0.002 && this._colors) {
            let strength = this._strength;
            this._beat = 0;
            if (this._heart) {
                // Only brightness changes, so it beats with motion reduced too.
                const t = now - this._heart.from;
                const settle = Math.min(1, Math.max(0, (t - HEART_S) / SETTLE_S));
                const beat = t < HEART_S + SETTLE_S ? heartbeat(t) : 0;
                this._beat = beat * (1 - settle);
                strength *= (BEAT_REST + (1 - BEAT_REST) * beat) * (1 - settle) + BEAT_SETTLED * settle;
            }
            if (this._breath) {
                // Only brightness changes, so it breathes with motion reduced too; speaking fills the breath in.
                const t = (now - this._breath.from) / BREATH_S;
                const dip = BREATH_DEPTH * (0.5 - 0.5 * Math.cos(2 * Math.PI * t)) * (1 - Math.min(1, this._mic * 1.5));
                strength *= 1 - dip;
            }
            this._paintAura(shape, ratio, strength);
            cr.setTransform(1, 0, 0, 1, 0, 0);
            cr.globalCompositeOperation = 'lighter';
            // A soft bloom outside first, then the light itself.
            cr.filter = `blur(${Math.round(6 * ratio)}px)`;
            cr.globalAlpha = 0.7;
            cr.drawImage(this._light, 0, 0);
            cr.filter = 'none';
            cr.globalAlpha = 1;
            cr.drawImage(this._light, 0, 0);
            cr.globalCompositeOperation = 'source-over';
            cr.setTransform(ratio, 0, 0, ratio, 0, 0);
        }
        for (const comet of this._comets) {
            const t = (now - comet.from) / COMET_S;
            if (comet.still)
                this._paintFlash(cr, shape, comet, t);
            else
                this._paintComet(cr, shape, comet, t);
        }
    }

    // The aura on its own layer (`paintRim`): faint strokes narrowing inwards, a haze along the bottom with two
    // brighter streaks passing each other, a crisp line at the very edge, all rising from the bottom.
    _paintAura(shape, ratio, strength) {
        const cr = this._light.getContext('2d');
        cr.setTransform(1, 0, 0, 1, 0, 0);
        cr.clearRect(0, 0, this._light.width, this._light.height);
        cr.setTransform(ratio, 0, 0, ratio, 0, 0);
        const glass = Boolean(this._island.edge);
        const steps = glass ? 6 : 3, mist = glass ? 1.05 : 1, bright = glass ? 1.3 : 1;
        // Each beat lifts the light higher, as well as brighter.
        const level = Math.max(0, this._level) + 0.6 * (this._beat ?? 0), work = this._work, time = this._flow;
        const left = shape.cx - shape.w / 2, right = shape.cx + shape.w / 2;
        const stroke = 22 + 16 * level, line = 2.5;
        const rise = Math.min(52, shape.h * 0.5) * (0.75 + 0.5 * level);
        const flow = (t, alpha, streaks) => {
            const gradient = cr.createLinearGradient(left, 0, right, 0);
            const n = this._colors.length;
            const shift = (t * FLOW_SPEED) % 1;
            const [s0, s1] = streaks === null ? [0, 0] : streakAt(streaks);
            for (let i = 0; i <= STOPS; i++) {
                const x = i / STOPS;
                const at = (x + shift) * n;
                const index = Math.floor(at), m = at - index;
                const from = this._colors[index % n], to = this._colors[(index + 1) % n];
                const swell = streaks === null ? 1
                    : 0.62 + (0.55 + 0.75 * work) * bump(x, s0, 0.09) + (0.4 + 0.55 * work) * bump(x, s1, 0.13);
                gradient.addColorStop(x, rgba(from.map((v, k) => v + (to[k] - v) * m), Math.min(1, alpha * swell)));
            }
            return gradient;
        };
        cr.globalCompositeOperation = 'lighter';
        cr.lineCap = 'round';
        cr.lineJoin = 'round';
        cr.strokeStyle = flow(time, (0.1 + 0.08 * level) * strength * 3 / steps, null);
        for (let i = 0; i < steps; i++) {
            cr.lineWidth = stroke * (1 - i / steps);
            traceSides(cr, shape);
            cr.stroke();
        }
        // The haze, inside the shape only, deepening towards the bottom.
        cr.save();
        cr.clip(new Path2D(this._island.path));
        cr.fillStyle = flow(time * 0.8 + 2, (0.32 + 0.24 * level) * strength * mist, time);
        const bands = 10;
        for (let k = 0; k < bands; k++) {
            cr.globalAlpha = (k + 0.5) / bands;
            cr.fillRect(left - 2, shape.h - rise + k * rise / bands, right - left + 4, rise / bands + 0.5);
        }
        cr.globalAlpha = 1;
        cr.restore();
        cr.strokeStyle = flow(time * 1.4, Math.min(1, (0.46 + 0.3 * level + 0.14 * work) * strength * bright), time + 3);
        cr.lineWidth = line;
        traceSides(cr, shape);
        cr.stroke();
        // Rising from the bottom: gone by the top of the sides.
        cr.globalCompositeOperation = 'destination-in';
        const mask = cr.createLinearGradient(0, 0, 0, shape.h);
        mask.addColorStop(0, 'rgba(0,0,0,0)');
        mask.addColorStop(RISE_KNEE, `rgba(0,0,0,${RISE_AT_KNEE})`);
        mask.addColorStop(1, 'rgba(0,0,0,1)');
        cr.fillStyle = mask;
        cr.fillRect(0, 0, window.innerWidth, window.innerHeight);
        cr.globalCompositeOperation = 'source-over';
    }

    // The edge lit all along at once, `t` (0..1) of the way through: in quickly, out slowly, never moving.
    _paintFlash(cr, shape, {rgb}, t) {
        const fade = Math.min(1, t / 0.15, (1 - t) / 0.6);
        cr.save();
        cr.globalCompositeOperation = 'lighter';
        cr.lineCap = 'round';
        cr.lineJoin = 'round';
        cr.shadowColor = rgba(rgb, fade);
        cr.shadowBlur = 18;
        cr.strokeStyle = rgba(rgb, 0.55 * fade);
        cr.lineWidth = 5;
        traceSides(cr, shape);
        cr.stroke();
        cr.shadowBlur = 0;
        cr.strokeStyle = rgba(rgb.map(v => v + (1 - v) * 0.5), 0.9 * fade);
        cr.lineWidth = 1.6;
        traceSides(cr, shape);
        cr.stroke();
        cr.restore();
    }

    // The comet `t` (0..1) of the way through its run: eased in and out, its tail running off the end.
    _paintComet(cr, shape, {rgb}, t) {
        const points = sidePoints(shape);
        const total = points.at(-1)[2];
        const tail = Math.min(240, Math.max(60, total * TAIL));
        const eased = t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2;
        const head = eased * (total + tail);
        // In and out softly at its ends.
        const fade = Math.min(1, t / 0.12, (1 - t) / 0.2);
        cr.save();
        cr.globalCompositeOperation = 'lighter';
        cr.lineCap = 'round';
        cr.shadowColor = rgba(rgb, fade);
        cr.shadowBlur = 18;
        // The tail: segments thinning and fading behind the head.
        let previous = null;
        for (const [x, y, along] of points) {
            const behind = head - along;
            if (behind < 0 || behind > tail) {
                previous = null;
                continue;
            }
            const k = behind / tail;
            if (previous) {
                // White-hot near the head, its own colour further back.
                cr.strokeStyle = rgba(rgb.map(v => v + (1 - v) * (1 - k) ** 3 * 0.6), (1 - k) ** 1.4 * fade);
                cr.lineWidth = 0.8 + 4 * (1 - k);
                cr.beginPath();
                cr.moveTo(previous[0], previous[1]);
                cr.lineTo(x, y);
                cr.stroke();
            }
            previous = [x, y];
        }
        cr.shadowBlur = 0;
        // The head: a white-hot core in a halo of its colour.
        const at = points.find(p => p[2] >= head);
        if (at) {
            const [x, y] = at;
            const halo = cr.createRadialGradient(x, y, 0, x, y, 46);
            halo.addColorStop(0, rgba(rgb, 0.65 * fade));
            halo.addColorStop(0.35, rgba(rgb.map(v => v * 0.75), 0.25 * fade));
            halo.addColorStop(1, rgba(rgb, 0));
            cr.fillStyle = halo;
            cr.fillRect(x - 46, y - 46, 92, 92);
            // A flare across the edge, as a hot light streaks in a lens.
            const flare = cr.createLinearGradient(x - 40, y, x + 40, y);
            flare.addColorStop(0, rgba(rgb, 0));
            flare.addColorStop(0.5, rgba(rgb.map(v => v + (1 - v) * 0.4), 0.7 * fade));
            flare.addColorStop(1, rgba(rgb, 0));
            cr.fillStyle = flare;
            cr.fillRect(x - 40, y - 1.2, 80, 2.4);
            const core = cr.createRadialGradient(x, y, 0, x, y, 11);
            core.addColorStop(0, `rgba(255, 255, 255, ${fade})`);
            core.addColorStop(0.3, `rgba(255, 235, 230, ${0.95 * fade})`);
            core.addColorStop(0.55, rgba(rgb, 0.85 * fade));
            core.addColorStop(1, rgba(rgb, 0));
            cr.fillStyle = core;
            cr.fillRect(x - 11, y - 11, 22, 22);
        }
        cr.restore();
    }
}
