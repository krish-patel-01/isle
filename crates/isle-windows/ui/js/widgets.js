// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The island's building blocks (gnome-extension/src/widgets.js) as DOM elements: icons, the progress ring,
// the music bars, cover art and the scrubber.

import {daemon} from './daemon.js';

/** Makes an element: `el('div', 'isle-x', [children])`. */
export function el(tag, className = '', children = []) {
    const node = document.createElement(tag);
    if (className)
        node.className = className;
    for (const child of [].concat(children))
        node.append(child);
    return node;
}

/** One of Isle's icons (gnome-extension/icons), drawn in the text's colour. */
export function icon(name, size = 16, className = '') {
    const node = el('span', `isle-icon ${className}`.trim());
    setIcon(node, name, size);
    return node;
}

export function setIcon(node, name, size = null) {
    node.style.setProperty('--icon', `url("icons/${name}.svg")`);
    if (size !== null) {
        node.style.width = `${size}px`;
        node.style.height = `${size}px`;
    }
}

/** A round button holding an icon. */
export function iconButton(name, className, onClick, {size = 16, label = ''} = {}) {
    const button = el('button', `isle-button ${className}`, [icon(name, size)]);
    if (label)
        button.setAttribute('aria-label', label);
    button.addEventListener('click', event => {
        event.stopPropagation();
        onClick();
    });
    return button;
}

export function setButtonIcon(button, name) {
    setIcon(button.firstChild, name);
}

/** A ring that fills clockwise from the top: the focus timer. */
export class Ring {
    constructor({size = 20, lineWidth = 3, trackOpacity = 0.25, child = null} = {}) {
        this.size = size;
        this.lineWidth = lineWidth;
        this.trackOpacity = trackOpacity;
        this.progress = 0;
        this.color = '#ffffff';
        this.node = el('div', 'isle-ring');
        this.node.style.width = this.node.style.height = `${size}px`;
        this._canvas = el('canvas');
        this.node.append(this._canvas);
        if (child)
            this.node.append(child);
        this.draw();
    }

    set(progress, color = this.color) {
        if (progress === this.progress && color === this.color)
            return;
        this.progress = progress;
        this.color = color;
        this.draw();
    }

    draw() {
        const ratio = window.devicePixelRatio || 1;
        const canvas = this._canvas;
        canvas.width = canvas.height = Math.round(this.size * ratio);
        canvas.style.width = canvas.style.height = `${this.size}px`;
        const cr = canvas.getContext('2d');
        cr.scale(ratio, ratio);
        const c = this.size / 2, r = c - this.lineWidth / 2;
        cr.lineWidth = this.lineWidth;
        cr.lineCap = 'round';
        cr.strokeStyle = `rgba(255, 255, 255, ${this.trackOpacity})`;
        cr.beginPath();
        cr.arc(c, c, r, 0, Math.PI * 2);
        cr.stroke();
        const p = Math.min(1, Math.max(0, this.progress));
        if (p > 0) {
            cr.strokeStyle = this.color;
            cr.beginPath();
            cr.arc(c, c, r, -Math.PI / 2, -Math.PI / 2 + p * Math.PI * 2);
            cr.stroke();
        }
    }
}

/** Four bars bouncing to the music; at rest, still at their own heights. */
export class Bars {
    constructor({height = 14, count = 4} = {}) {
        this.node = el('div', 'isle-bars');
        this.node.style.height = `${height}px`;
        this._height = height;
        this._playing = false;
        this._bars = [];
        this._bounces = [];
        for (let i = 0; i < count; i++) {
            const bar = el('div', 'isle-bar');
            bar.style.height = `${height}px`;
            this._bars.push(bar);
            this._bounces.push({scale: this._rest(i), from: 0, to: 0, start: 0, duration: 0});
            this.node.append(bar);
        }
        this._apply();
        this._frame = 0;
    }

    _rest(index) {
        return [0.45, 1, 0.7, 0.35, 0.8][index % 5];
    }

    set playing(playing) {
        if (playing === this._playing)
            return;
        this._playing = playing;
        if (playing) {
            const now = performance.now();
            this._bounces.forEach(b => this._next(b, now));
            if (!this._frame)
                this._frame = requestAnimationFrame(t => this._tick(t));
        } else {
            cancelAnimationFrame(this._frame);
            this._frame = 0;
            this._bars.forEach((bar, i) => {
                this._bounces[i].scale = this._rest(i);
                bar.style.transition = 'transform 200ms';
            });
            this._apply();
        }
    }

    set opacity(value) {
        this.node.style.opacity = value;
    }

    _next(bounce, now) {
        bounce.from = bounce.scale;
        bounce.to = 0.25 + Math.random() * 0.75;
        bounce.start = now;
        bounce.duration = 180 + Math.random() * 220;
    }

    _tick(now) {
        for (const bounce of this._bounces) {
            const t = Math.min(1, (now - bounce.start) / bounce.duration);
            const eased = t < 0.5 ? 2 * t * t : 1 - (-2 * t + 2) ** 2 / 2;
            bounce.scale = bounce.from + (bounce.to - bounce.from) * eased;
            if (t >= 1)
                this._next(bounce, now);
        }
        this._bars.forEach(bar => (bar.style.transition = 'none'));
        this._apply();
        this._frame = this._playing ? requestAnimationFrame(t => this._tick(t)) : 0;
    }

    _apply() {
        this._bars.forEach((bar, i) => (bar.style.transform = `scaleY(${this._bounces[i].scale})`));
    }
}

/** Cover art: the track's picture, or a music note on grey until there is one. */
export class Art {
    constructor({size = 24, radius = 6} = {}) {
        this.node = el('div', 'isle-art', [icon('isle-music', Math.round(size * 0.42))]);
        this.node.style.width = this.node.style.height = `${size}px`;
        this.node.style.borderRadius = `${radius}px`;
        this._url = null;
    }

    set url(url) {
        if (url === this._url)
            return;
        this._url = url;
        this.node.style.backgroundImage = url ? `url("${url}")` : '';
        this.node.classList.toggle('shown', Boolean(url));
    }
}

/** A scrubber: drag or click to set a value in 0..1. */
export class Slider {
    constructor(onSet) {
        this.node = el('div', 'isle-slider', [
            el('div', 'isle-slider-track'),
            this._fill = el('div', 'isle-slider-fill'),
            this._knob = el('div', 'isle-slider-knob'),
        ]);
        this.dragging = false;
        this._value = 0;
        const valueAt = event => {
            const box = this.node.getBoundingClientRect();
            return Math.min(1, Math.max(0, (event.clientX - box.left) / box.width));
        };
        this.node.addEventListener('pointerdown', event => {
            this.dragging = true;
            this.node.classList.add('dragging');
            this.node.setPointerCapture(event.pointerId);
            this.value = valueAt(event);
        });
        this.node.addEventListener('pointermove', event => {
            if (this.dragging)
                this.value = valueAt(event);
        });
        this.node.addEventListener('pointerup', event => {
            if (!this.dragging)
                return;
            this.dragging = false;
            this.node.classList.remove('dragging');
            onSet(valueAt(event));
        });
    }

    set value(value) {
        this._value = value;
        const pct = `${(value * 100).toFixed(3)}%`;
        this._fill.style.width = pct;
        this._knob.style.left = pct;
    }

    get value() {
        return this._value;
    }

    set enabled(enabled) {
        this.node.classList.toggle('disabled', !enabled);
    }
}

/** "3:07", or "1:02:03" past an hour. */
export function formatTime(ms) {
    const total = Math.max(0, Math.floor(ms / 1000));
    const h = Math.floor(total / 3600), m = Math.floor(total / 60) % 60, s = total % 60;
    const ss = String(s).padStart(2, '0');
    return h ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

/** Whether times show with a 12-hour clock: as Isle's setting says (clock-format), else as Windows does. */
export function uses12Hour() {
    const settings = daemon.state.settings;
    const format = settings?.['clock-format'];
    if (format === '12h' || format === '24h')
        return format === '12h';
    if (typeof settings?.['system-12h'] === 'boolean')
        return settings['system-12h'];
    return new Intl.DateTimeFormat(undefined, {hour: 'numeric'}).resolvedOptions().hour12 === true;
}

/** "14:05", or "2:05 PM" with a 12-hour clock: a time of day that stands alone. */
export function timeOfDay(date) {
    const {time, meridiem} = clockParts(date);
    return meridiem ? `${time} ${meridiem}` : time;
}

/** The clock as {time, meridiem}: "9:41" and "AM", or "09:41" and "". */
export function clockParts(date = new Date()) {
    if (!uses12Hour())
        return {time: `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`, meridiem: ''};
    const hours = date.getHours() % 12 || 12;
    return {time: `${hours}:${String(date.getMinutes()).padStart(2, '0')}`, meridiem: date.getHours() < 12 ? 'AM' : 'PM'};
}

/** "Tue 3 Nov", as the header shows the date. */
export function longDate(date = new Date()) {
    const day = date.toLocaleDateString('en-GB', {weekday: 'short'});
    const month = date.toLocaleDateString('en-GB', {month: 'short'});
    return `${day} ${date.getDate()} ${month}`;
}

/** Calls `fn` now and at the start of every minute. Returns a function that stops it. */
export function watchClock(fn) {
    fn();
    let timer = 0;
    const schedule = () => {
        const now = new Date();
        const wait = 60000 - (now.getSeconds() * 1000 + now.getMilliseconds()) + 20;
        timer = setTimeout(() => {
            fn();
            schedule();
        }, wait);
    };
    schedule();
    return () => clearTimeout(timer);
}

/** Where the song is now, in µs: the last sample moved on by the time since, while it plays. */
export function positionNow(np) {
    if (!np?.available)
        return 0;
    let position = np.position_us;
    if (np.status === 'Playing')
        position += (Date.now() - np.sampled_at_ms) * 1000 * (np.rate || 1);
    return np.length_us > 0 ? Math.min(position, np.length_us) : position;
}

/** A track to show: playing or paused, unless a browser's that is over. */
export function hasTrack(np) {
    return Boolean(np?.available && np.title);
}

export function showsClosed(np) {
    return hasTrack(np) && !np.resting && np.status !== 'Stopped';
}
