// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The closed island (gnome-extension/src/compact.js), hanging from the top of the screen: the time in the
// middle and what is going on either side of it: a song's cover and bars, or the focus timer's ring and
// time left. With both, the song keeps its sides and the timer is a small ring beside the bars. A call comes
// first: a green phone on the left, the next activity's sign on the right, or a red microphone while muted.

import {daemon} from './daemon.js';
import {Art, Bars, Ring, clockParts, el, formatTime, icon, showsClosed, watchClock} from './widgets.js';

/** CSS pixels, as SIZES.hanging in compact.js: the island with only the clock, and its signs. */
const SIZE = {clock: 164, art: 24, artRadius: 6, ring: 20, bars: 16, call: 15};
const PILL_HEIGHT = 30;
const CORNER_RADIUS = 16;
/** How far in from the island's end text on the right sits. */
const TEXT_INSET = 13;
/** Each side's gap toward the clock (.isle-compact-start / -end). */
const SIDE_GAP = 30;
const PAUSED_OPACITY = 110 / 255;
const GREEN = '#30d158';

export function timerColor(phase) {
    return phase === 'focus' || phase === 'idle' ? '#ffffff' : GREEN;
}

export class CompactView {
    constructor(onWidth) {
        this._onWidth = onWidth;
        this.node = el('div', 'isle-view isle-compact');
        this._art = new Art({size: SIZE.art, radius: SIZE.artRadius});
        this._ring = new Ring({size: SIZE.ring, lineWidth: 3});
        this._secondRing = new Ring({size: SIZE.ring, lineWidth: 3});
        this._bars = new Bars({height: SIZE.bars});
        this._timerTime = el('span', 'isle-compact-second-time');
        this._secondArt = new Art({size: SIZE.art, radius: SIZE.artRadius});
        this._phone = icon('isle-phone-symbolic', SIZE.call, 'isle-call-icon');
        this._micOff = icon('isle-mic-off-symbolic', SIZE.call, 'isle-call-muted');

        this._left = el('div', 'isle-compact-slot isle-compact-start');
        this._right = el('div', 'isle-compact-slot isle-compact-end');
        this._clockTime = el('span', 'isle-compact-clock');
        this._meridiem = el('span', 'isle-compact-meridiem');
        const clock = el('div', 'isle-compact-clock-row', [this._clockTime, this._meridiem]);
        this._centre = el('div', 'isle-compact-center', [clock]);
        this.node.append(this._left, this._centre, this._right);

        this._width = SIZE.clock;
        this._artKey = null;
        this._ticker = 0;
        watchClock(() => this._tickClock());
        daemon.connect(key => {
            if (key === 'nowPlaying' || key === 'pomodoro' || key === 'call')
                this.refresh();
            // The clock's format, the island's height, a page left out: laid out anew.
            if (key === 'settings') {
                this._tickClock();
                this.refresh();
                this._onWidth?.();
            }
        });
        this.refresh();
    }

    get preferredWidth() {
        return this._width;
    }

    /** As tall as Settings say (Island height, notch-depth); 30 px as Isle comes. */
    get height() {
        return daemon.state.settings?.['notch-depth'] ?? PILL_HEIGHT;
    }

    get radius() {
        return Math.min(CORNER_RADIUS, this.height / 2);
    }

    _tickClock() {
        const {time, meridiem} = clockParts();
        this._clockTime.textContent = time;
        this._meridiem.textContent = meridiem;
        this._meridiem.style.display = meridiem ? '' : 'none';
        this._fit();
    }

    /** The cover of the playing song, fetched once per track. */
    async _syncArt(np) {
        if (np.art_key === this._artKey)
            return;
        this._artKey = np.art_key;
        this._art.url = this._secondArt.url = null;
        const url = await daemon.art();
        if (this._artKey === np.art_key) {
            this._art.url = this._secondArt.url = url;
            this.artUrl = url;
            this.onArt?.(url);
        }
    }

    refresh() {
        const np = daemon.state.nowPlaying;
        const timer = daemon.state.pomodoro;
        // The Music page left out in Settings takes the song out of the closed island too.
        const music = showsClosed(np) && !daemon.state.settings?.['hidden-pages']?.includes('music');
        const timing = timer && timer.phase !== 'idle' && (timer.running || timer.started_at_ms);
        const call = daemon.state.call;
        const onCall = call?.active && daemon.state.settings?.['call-activity'] !== false;
        let left = null, right = null;
        this._timing = timing;

        if (onCall) {
            // The call takes the left; the next activity's sign the right, a muted microphone over it.
            this._bars.playing = false;
            if (music) {
                this._syncArt(np);
                right = this._secondArt.node;
            } else if (timing) {
                right = this._secondRing.node;
            }
            left = this._phone;
            if (call.muted)
                right = this._micOff;
        } else if (music) {
            this._syncArt(np);
            left = this._art.node;
            this._bars.playing = np.status === 'Playing';
            this._bars.opacity = np.status === 'Playing' ? 1 : PAUSED_OPACITY;
            if (timing) {
                right = el('div', 'row', [this._secondRing.node]);
                right.style.gap = '6px';
                right.prepend(this._bars.node);
            } else {
                right = this._bars.node;
            }
        } else {
            this._bars.playing = false;
            if (timing) {
                left = this._ring.node;
                right = this._timerTime;
            }
        }
        this._left.replaceChildren(...(left ? [left] : []));
        this._right.replaceChildren(...(right ? [right] : []));
        this._syncTimer();
        clearInterval(this._ticker);
        if (timing && timer.running)
            this._ticker = setInterval(() => this._syncTimer(), 1000);
        this._fit();
    }

    _syncTimer() {
        const timer = daemon.state.pomodoro;
        if (!timer || timer.phase === 'idle')
            return;
        const remaining = timer.running ? Math.max(0, timer.ends_at_ms - Date.now()) : timer.remaining_ms;
        const progress = timer.duration_ms ? 1 - remaining / timer.duration_ms : 0;
        const color = timerColor(timer.phase);
        this._ring.set(progress, color);
        this._secondRing.set(progress, color);
        this._timerTime.textContent = formatTime(remaining + 999);
        this._timerTime.style.color = color;
        this._timerTime.style.opacity = timer.running ? 1 : PAUSED_OPACITY;
    }

    /** Both sides as wide as the wider (its sign, the inset from the end, and its gap to the clock). */
    _fit() {
        const hasLeft = this._left.childElementCount > 0, hasRight = this._right.childElementCount > 0;
        let width = SIZE.clock;
        if (hasLeft || hasRight) {
            const inset = Math.max(Math.round((this.height - SIZE.art) / 2), this.radius - SIZE.artRadius);
            const measure = node => node.firstChild ? Math.ceil(node.firstChild.getBoundingClientRect().width) : 0;
            const rightInset = this._right.firstChild === this._timerTime ? TEXT_INSET : inset;
            const side = Math.max(measure(this._left) + inset, measure(this._right) + rightInset) + SIDE_GAP;
            this._left.style.width = this._right.style.width = `${side}px`;
            this._left.style.paddingLeft = `${inset}px`;
            this._right.style.paddingRight = `${rightInset}px`;
            const clock = Math.ceil(this._centre.firstChild.getBoundingClientRect().width);
            width = Math.max(SIZE.clock, clock + 2 * side);
        } else {
            this._left.style.width = this._right.style.width = '0';
            this._left.style.paddingLeft = this._right.style.paddingRight = '0';
        }
        // An even width: centred to the pixel.
        width = 2 * Math.round(width / 2);
        for (const slot of [this._left, this._right])
            slot.style.boxSizing = 'border-box';
        if (width !== this._width) {
            this._width = width;
            this._onWidth?.();
        }
    }
}
