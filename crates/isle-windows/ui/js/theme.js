// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The island's look (gnome-extension/src/theme.js): Isle's own black, or glass over what is behind it. On GNOME the
// glass blurs what is behind; Windows offers no blur an always-on-top, never-active window can cut to the island's
// shape, so here the glass is a deep smoke the colours behind show through softly, as Isle's does over a wallpaper.

const BLACK = {fill: [0, 0, 0, 1], edge: null, sheen: 0};
// Clear glass: a deep cool smoke, a faint rim lit from above, a sheen.
const CLEAR = {fill: [0.1, 0.1, 0.16, 0.86], edge: {top: 0.13, bottom: 0.05}, sheen: 0.06};
// Glass at its darkest (Settings' tint at 100): dark smoked glass, little colour through.
const DARK = {fill: [0.04, 0.045, 0.06, 0.93], edge: {top: 0.07, bottom: 0.04}, sheen: 0};

const mix = (a, b, t) => a + (b - a) * t;

let current = BLACK;
let glass = false;

/** Sets the look by its Settings name (anything unknown is black), glass `tint` 0 (clear) to 100 (dark). */
export function setTheme(name, tint = 0) {
    glass = name === 'glass';
    if (!glass) {
        current = BLACK;
        return;
    }
    const t = Math.min(1, Math.max(0, tint / 100));
    // Not one blend: the smoke's grey comes early and the sheen goes early, so along the slider the glass only
    // ever greys and settles.
    const early = Math.sqrt(t);
    current = {
        fill: CLEAR.fill.map((v, i) => mix(v, DARK.fill[i], i < 3 ? early : t)),
        edge: {top: mix(CLEAR.edge.top, DARK.edge.top, t), bottom: mix(CLEAR.edge.bottom, DARK.edge.bottom, t)},
        sheen: mix(CLEAR.sheen, DARK.sheen, early),
    };
}

/** Whether the island shows what is behind it (glass). */
export function translucent() {
    return glass;
}

/** The island's colour as RGBA in 0..1. */
export function fill() {
    return current.fill;
}

/** The rim of light round the island ({top, bottom}: its white's alpha there), or null. */
export function edge() {
    return current.edge;
}

/** How much lighter the island's top is than its bottom. */
export function sheen() {
    return current.sheen;
}
