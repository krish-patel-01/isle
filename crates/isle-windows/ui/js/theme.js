// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The island's look (gnome-extension/src/theme.js): Isle's own black, or glass over what is behind it. The glass
// is GNOME's: what is behind blurred and toned in one pass (glass.js), its lightness mapped to lo + span × l^gamma,
// its colour kept × saturation (`small` on a closed island, `large` open), a sheen and a glow added, the edge bending
// what is behind and catching the light. Where Isle cannot see what is behind (glass.rs), the glass is a deep smoke
// instead, the colours behind showing through softly.

// Clear glass, thick at its edge, and glass at its darkest (Settings' tint at 100): GNOME's values exactly.
const GLASS = {fill: [0.03, 0.04, 0.05, 0.22],
    edge: {top: 0.16, bottom: 0.1},
    blur: {radius: 14, small: 0.95, large: 1.15, lo: 0.03, span: 0.64, gamma: 1, deep: 0.52,
        knee: 0.22, sheen: 0.03, glow: 0.05, lens: 26, band: 24, spread: 0.7, rim: 0.5, rimWidth: 1.1}};
const DARK_GLASS = {fill: [0.02, 0.025, 0.03, 0.3],
    edge: {top: 0.07, bottom: 0.04},
    blur: {radius: 40, small: 0.7, large: 0.85, lo: 0, span: 0.48, gamma: 1, deep: 0.25, knee: 0.55, sheen: 0, glow: 0,
        lens: 0, band: 20, spread: 0, rim: 0, rimWidth: 1.1}};

// Without a backdrop: a deep cool smoke, a faint rim lit from above, a sheen; at its darkest, little colour through.
const SMOKE = {fill: [0.1, 0.1, 0.16, 0.86], edge: {top: 0.13, bottom: 0.05}, sheen: 0.06};
const DARK_SMOKE = {fill: [0.04, 0.045, 0.06, 0.93], edge: {top: 0.07, bottom: 0.04}, sheen: 0};

const BLACK = {fill: [0, 0, 0, 1], edge: null, sheen: 0, blur: null};

const mix = (a, b, t) => a + (b - a) * t;

let glass = false;
let backdrop = false;
let tinted = BLACK;
let smoke = BLACK;

/** Sets the look by its Settings name (anything unknown is black), glass `tint` 0 (clear) to 100 (dark). */
export function setTheme(name, tint = 0) {
    glass = name === 'glass';
    if (!glass) {
        tinted = smoke = BLACK;
        return;
    }
    const t = Math.min(1, Math.max(0, tint / 100));
    // Not one blend: the smoke's grey comes early and the sheen goes early, the backdrop's tone late, so along the
    // slider the glass only ever greys and settles (a plain blend dipped darker than both ends halfway).
    const early = Math.sqrt(t), late = t * t;
    const ease = {sheen: early, glow: early};
    const edge = (a, b) => ({top: mix(a.edge.top, b.edge.top, t), bottom: mix(a.edge.bottom, b.edge.bottom, t)});
    tinted = {
        fill: GLASS.fill.map((v, i) => mix(v, DARK_GLASS.fill[i], i < 3 ? early : t)),
        edge: edge(GLASS, DARK_GLASS),
        sheen: 0,
        blur: {...GLASS.blur, ...Object.fromEntries(Object.entries(DARK_GLASS.blur)
            .map(([k, v]) => [k, mix(GLASS.blur[k], v, ease[k] ?? late)]))},
    };
    smoke = {
        fill: SMOKE.fill.map((v, i) => mix(v, DARK_SMOKE.fill[i], i < 3 ? early : t)),
        edge: edge(SMOKE, DARK_SMOKE),
        sheen: mix(SMOKE.sheen, DARK_SMOKE.sheen, early),
        blur: null,
    };
}

/** Whether what is behind the island can be shown blurred (a picture of it has come). */
export function setBackdrop(on) {
    backdrop = on;
}

const look = () => (backdrop ? tinted : smoke);

/** Whether the island shows what is behind it (glass). */
export function translucent() {
    return glass;
}

/** The island's colour as RGBA in 0..1, over the blurred backdrop or the smoke's own. */
export function fill() {
    return look().fill;
}

/** The rim of light round the island ({top, bottom}: its white's alpha there), or null. */
export function edge() {
    return look().edge;
}

/** How much lighter the island's top is than its bottom (the backdrop's sheen is in its shader). */
export function sheen() {
    return look().sheen;
}

/** How what is behind the island is blurred and toned (glass.js), or null for not at all. */
export function blur() {
    return glass && backdrop ? tinted.blur : null;
}
