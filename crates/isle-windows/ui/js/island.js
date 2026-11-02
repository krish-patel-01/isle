// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The island itself (gnome-extension/src/island.js): a black shape hanging from the top of the screen that
// springs between sizes and cross-fades views. Its outline is the shader's: a body with rounded bottom
// corners, and shoulders that flare from its sides into the screen's top edge.

import {GENTLE, Spring, SpringDriver} from './spring.js';

/** Cross-fade timings in milliseconds. */
const FADE_OUT = 160;
const FADE_IN = 320;
/** The incoming view waits for the shape to make room before appearing. */
const FADE_IN_DELAY = 120;
/** How far (CSS px) the views slide as the shape grows or shrinks: they travel with it. */
const VIEW_SHIFT = 8;
/** Below this shoulder size (CSS px) the shoulders fade out, so a small island rounds smoothly. */
const SHOULDER_FADE = 4;

/** The island's outline as an SVG path, `w` × `h` centred on `cx`, corners `r`, shoulders `s`. */
export function outline(cx, w, h, r, s) {
    w = Math.max(0, w);
    h = Math.max(0, h);
    r = Math.max(0, Math.min(r, w / 2, h));
    s = Math.max(0, Math.min(s, h - r));
    if (s < SHOULDER_FADE)
        s *= s / SHOULDER_FADE;
    const left = cx - w / 2, right = cx + w / 2;
    const f = n => n.toFixed(2);
    return [
        `M ${f(left - s)} 0`,
        s > 0 ? `A ${f(s)} ${f(s)} 0 0 1 ${f(left)} ${f(s)}` : '',
        `L ${f(left)} ${f(h - r)}`,
        r > 0 ? `A ${f(r)} ${f(r)} 0 0 0 ${f(left + r)} ${f(h)}` : '',
        `L ${f(right - r)} ${f(h)}`,
        r > 0 ? `A ${f(r)} ${f(r)} 0 0 0 ${f(right)} ${f(h - r)}` : '',
        `L ${f(right)} ${f(s)}`,
        s > 0 ? `A ${f(s)} ${f(s)} 0 0 1 ${f(right + s)} 0` : '',
        'Z',
    ].join(' ');
}

export class Island {
    /** `root` holds a `<canvas id=shape>` and a `<div id=views>`. */
    constructor(root) {
        this._canvas = root.querySelector('#shape');
        this._views = root.querySelector('#views');
        this._width = new Spring(0);
        this._height = new Spring(0);
        this._radius = new Spring(0, GENTLE);
        this._shoulder = new Spring(0, GENTLE);
        this._springs = [this._width, this._height, this._radius, this._shoulder];
        this._driver = new SpringDriver(this._springs, () => this._sync());
        this._view = null;
        this.fill = [0, 0, 0, 1];
        this.onFrame = null;
        window.addEventListener('resize', () => this._sync());
    }

    get centre() {
        return window.innerWidth / 2;
    }

    /** The shape as it is now: {x, y, width, height}, the shoulders' band included. */
    get rect() {
        const w = Math.max(0, this._width.value), s = Math.max(0, this._shoulder.value);
        return {x: this.centre - w / 2 - s, y: 0, width: w + 2 * s, height: Math.max(0, this._height.value)};
    }

    /** The shape it is heading to. */
    get targetRect() {
        const w = this._width.target, s = this._shoulder.target;
        return {x: this.centre - w / 2 - s, y: 0, width: w + 2 * s, height: this._height.target};
    }

    /** Whether the point (CSS px, in the window) is on the island, as it is now or will be. */
    contains(x, y, slack = 0) {
        const inside = r => x >= r.x - slack && x <= r.x + r.width + slack && y >= -slack && y <= r.height + slack;
        return inside(this.rect) || (this._driver.running && inside(this.targetRect));
    }

    get morphing() {
        return this._driver.running;
    }

    /** Animates to `size` ({width, height, radius, shoulder}) and shows `view` (a DOM node) in it. */
    morph(size, view) {
        const growing = size.height + size.width > this._height.target + this._width.target;
        if (this._width.target === 0 && this._height.target === 0) {
            // The first shape appears at once.
            for (const [spring, value] of [[this._width, size.width], [this._height, size.height],
                [this._radius, size.radius], [this._shoulder, size.shoulder]])
                spring.reset(value);
        } else {
            this._width.setTarget(size.width);
            this._height.setTarget(size.height);
            this._radius.setTarget(size.radius);
            this._shoulder.setTarget(size.shoulder);
        }
        view.style.width = `${size.width}px`;
        view.style.height = `${size.height}px`;
        if (view !== this._view)
            this._swap(view, growing);
        this._driver.start();
        this._sync();
    }

    /** Cross-fades from the shown view to `view`, both travelling a little with the shape. */
    _swap(view, growing) {
        const old = this._view;
        this._view = view;
        const shift = growing ? VIEW_SHIFT : -VIEW_SHIFT;
        if (old) {
            old.style.transition = `opacity ${FADE_OUT}ms ease-out, transform ${FADE_OUT}ms ease-out`;
            old.style.opacity = '0';
            old.style.transform = `translateY(${shift}px)`;
            old.style.pointerEvents = 'none';
            clearTimeout(old._hide);
            old._hide = setTimeout(() => {
                if (old !== this._view)
                    old.style.visibility = 'hidden';
            }, FADE_OUT);
        }
        clearTimeout(view._hide);
        view.style.visibility = 'visible';
        view.style.pointerEvents = 'auto';
        view.style.transition = 'none';
        view.style.opacity = '0';
        view.style.transform = `translateY(${-shift}px)`;
        // Two frames: the start state must be drawn before the transition to the end state begins.
        requestAnimationFrame(() => requestAnimationFrame(() => {
            if (view !== this._view)
                return;
            view.style.transition = `opacity ${FADE_IN}ms ease-out ${old ? FADE_IN_DELAY : 0}ms, ` +
                `transform ${FADE_IN}ms cubic-bezier(0.22, 1, 0.36, 1) ${old ? FADE_IN_DELAY : 0}ms`;
            view.style.opacity = '1';
            view.style.transform = 'translateY(0)';
        }));
    }

    /** Draws the shape where the springs are, and cuts the views to it. */
    _sync() {
        const ratio = window.devicePixelRatio || 1;
        const width = window.innerWidth, height = window.innerHeight;
        const canvas = this._canvas;
        if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) {
            canvas.width = Math.round(width * ratio);
            canvas.height = Math.round(height * ratio);
        }
        const path = outline(this.centre, this._width.value, this._height.value, this._radius.value, this._shoulder.value);
        const cr = canvas.getContext('2d');
        cr.setTransform(1, 0, 0, 1, 0, 0);
        cr.clearRect(0, 0, canvas.width, canvas.height);
        cr.setTransform(ratio, 0, 0, ratio, 0, 0);
        const [r, g, b, a] = this.fill;
        cr.fillStyle = `rgba(${r * 255}, ${g * 255}, ${b * 255}, ${a})`;
        cr.fill(new Path2D(path));
        this._views.style.clipPath = `path('${path}')`;
        // Each view stays put on screen, centred, as the shape moves around it.
        for (const view of this._views.children)
            view.style.left = `${Math.round(this.centre - parseFloat(view.style.width || '0') / 2)}px`;
        this.onFrame?.();
    }
}
