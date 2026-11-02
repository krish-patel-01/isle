// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// Damped springs drive every island animation: unlike fixed-duration easing, a spring can be
// retargeted mid-flight and keeps its velocity, so the island never jolts.
// (gnome-extension/src/spring.js, stepped by the browser's frames instead of a Clutter timeline.)

/** The island's size spring: ~3.5% overshoot, peaking at ~0.37 s and at rest by ~0.57 s. */
export const ISLAND = {stiffness: 155, damping: 18};
/** Same speed without overshoot, for corner and shoulder radii. */
export const GENTLE = {stiffness: 155, damping: 25};

const MAX_STEP = 1 / 240;
const MAX_FRAME = 0.25;
/** At rest within a quarter pixel, slower than 2 px/s. */
const REST_DISTANCE = 0.25;
const REST_SPEED = 2;

/** One animated value pulled toward its target by a damped spring. */
export class Spring {
    constructor(value, config = ISLAND) {
        this.value = value;
        this.target = value;
        this.velocity = 0;
        this.config = config;
    }

    setTarget(target, config = this.config) {
        this.target = target;
        this.config = config;
    }

    reset(value) {
        this.value = this.target = value;
        this.velocity = 0;
    }

    get settled() {
        return Math.abs(this.value - this.target) < REST_DISTANCE &&
            Math.abs(this.velocity) < REST_SPEED;
    }

    /** Advances by `dt` seconds; returns whether the spring is at rest. */
    step(dt) {
        let remaining = Math.min(Math.max(dt, 0), MAX_FRAME);
        while (remaining > 0 && !this.settled) {
            const h = Math.min(remaining, MAX_STEP);
            const {stiffness, damping} = this.config;
            const acceleration = -stiffness * (this.value - this.target) - damping * this.velocity;
            this.velocity += acceleration * h;
            this.value += this.velocity * h;
            remaining -= h;
        }
        if (this.settled) {
            this.value = this.target;
            this.velocity = 0;
            return true;
        }
        return false;
    }
}

/** Steps springs once per frame, calling `onFrame` each step, until all rest. */
export class SpringDriver {
    constructor(springs, onFrame) {
        this._springs = springs;
        this._onFrame = onFrame;
        this._frame = 0;
        this._last = 0;
    }

    get running() {
        return this._frame !== 0;
    }

    start() {
        if (this._frame)
            return;
        this._last = performance.now();
        this._frame = requestAnimationFrame(now => this._tick(now));
    }

    _tick(now) {
        const dt = (now - this._last) / 1000;
        this._last = now;
        let settled = true;
        for (const spring of this._springs)
            settled = spring.step(dt) && settled;
        this._onFrame();
        this._frame = settled ? 0 : requestAnimationFrame(next => this._tick(next));
    }
}
