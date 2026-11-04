// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The call going on (gnome-extension/src/pages/call.js), with round controls: a live preview of the call's window
// (you and whoever you call), its app and duration, mute, and a way back. It only has a tab during a call.
//
// The preview is Windows' own copy of the window (a DWM thumbnail, calls.rs), drawn over the island where this
// page leaves room for it, once the page holds still: nothing is captured, and it only exists while it is shown.

import {daemon} from '../daemon.js';
import {el, icon, setIcon} from '../widgets.js';

const {invoke} = window.__TAURI__.core;

/** Sizes in CSS pixels, as call.js has them. */
const PREVIEW_HEIGHT = 128;
/** The preview takes the window's shape within these widths, cropping what is wider. */
const PREVIEW_MIN_WIDTH = 168;
const PREVIEW_MAX_WIDTH = 228;
/** The preview's width while it shows only the app's icon. */
const PLACEHOLDER_WIDTH = 196;
/** The preview's place must hold still this many frames (the island opening, the page sliding in) to be shown. */
const STILL_FRAMES = 4;

/** Formats a duration as "3:07" or "1:02:07" (format.js `duration`). */
export function duration(ms) {
    const total = Math.max(0, Math.round(ms / 1000));
    const hours = Math.floor(total / 3600);
    const minutes = Math.floor(total / 60) % 60;
    const seconds = String(total % 60).padStart(2, '0');
    return hours > 0 ? `${hours}:${String(minutes).padStart(2, '0')}:${seconds}` : `${minutes}:${seconds}`;
}

/** The call's app icon, or a phone. */
function appGlyph(call, size) {
    if (call?.icon) {
        const image = el('img', 'isle-call-app-icon');
        image.src = call.icon;
        image.width = image.height = size;
        return image;
    }
    return icon('isle-phone-symbolic', size);
}

export class CallPage {
    constructor() {
        this.name = 'call';
        this.node = el('div', 'isle-page isle-call');
        this.onClose = null;

        this._appIcon = el('div', 'isle-call-placeholder-icon');
        this._hint = el('div', 'isle-call-hint');
        this._placeholder = el('div', 'isle-call-placeholder', [this._appIcon, this._hint]);
        this._preview = el('button', 'isle-call-preview', [this._placeholder,
            el('div', 'isle-call-preview-hover', [icon('isle-open-symbolic', 16)])]);
        this._preview.setAttribute('aria-label', 'Go to the call');
        this._preview.addEventListener('click', () => this._open());

        this._headIcon = el('div', 'isle-call-head-icon');
        this._name = el('span', 'isle-call-name');
        this._kind = el('span', 'isle-call-kind');
        this._duration = el('span', 'isle-call-duration', ['0:00']);
        this._mute = this._control('isle-mic-symbolic', 'Mute', () => invoke('call_mute'));
        this._show = this._control('isle-open-symbolic', 'Open', () => this._open());
        const side = el('div', 'isle-call-side', [
            el('div', 'isle-call-head', [this._headIcon, this._name]),
            el('div', 'isle-call-status', [this._kind, el('span', 'isle-call-kind', ['·']), this._duration]),
            el('div', 'isle-call-controls', [this._mute.box, this._show.box]),
        ]);
        this.node.append(this._preview, side);

        this._iconKey = null;
        this._active = false;
        daemon.connect(() => this._update(), 'call');
        daemon.connect(() => this._update(), 'settings');
        this._update();
    }

    get preferredWidth() {
        return null;
    }

    /** A round button with its caption below. */
    _control(iconName, caption, onClick) {
        const button = el('button', 'isle-call-control', [icon(iconName, 18)]);
        button.setAttribute('aria-label', caption);
        button.addEventListener('click', event => {
            event.stopPropagation();
            onClick();
        });
        const text = el('span', 'isle-call-control-label', [caption]);
        return {box: el('div', 'isle-call-control-box', [button, text]), button, text};
    }

    /** Raises the call's window and closes the island. */
    _open() {
        invoke('call_raise');
        this.onClose?.();
    }

    /** Whether the live preview may show: during a call with a window, as Settings allow. */
    get _live() {
        const call = daemon.state.call;
        return this._active && Boolean(call?.active && call.reachable) && daemon.state.settings?.['call-preview'] !== false;
    }

    /** Redraws from the call's state. */
    _update() {
        const call = daemon.state.call;
        if (!call?.active) {
            this._stopPreview();
            return;
        }
        const key = call.icon || 'phone';
        if (key !== this._iconKey) {
            this._iconKey = key;
            this._appIcon.replaceChildren(appGlyph(call, 40));
            this._headIcon.replaceChildren(appGlyph(call, 20));
        }
        this._name.textContent = call.name;
        this._kind.textContent = call.video ? 'Video call' : 'Voice call';

        const muted = call.muted;
        setIcon(this._mute.button.firstChild, muted ? 'isle-mic-off-symbolic' : 'isle-mic-symbolic');
        this._mute.button.classList.toggle('checked', muted);
        this._mute.text.textContent = muted ? 'Muted' : 'Mute';
        this._mute.button.setAttribute('aria-label', muted ? 'Unmute the microphone' : 'Mute the microphone');

        this._show.button.disabled = !call.reachable;
        this._preview.disabled = !call.reachable;
        // As wide as the window's shape allows; the icon's width without one.
        const width = this._live && call.aspect > 0
            ? Math.min(PREVIEW_MAX_WIDTH, Math.max(PREVIEW_MIN_WIDTH, Math.round(PREVIEW_HEIGHT * call.aspect)))
            : PLACEHOLDER_WIDTH;
        this._preview.style.width = `${width}px`;
        this._hint.textContent = '';
        this._tick();
        if (this._live)
            this._startPreview();
        else
            this._stopPreview();
    }

    /** Shows the call's length, just after each second turns. */
    _tick() {
        clearTimeout(this._ticker);
        const call = daemon.state.call;
        if (!call?.active)
            return;
        const elapsed = Date.now() - call.since_ms;
        this._duration.textContent = duration(elapsed);
        if (this._active)
            this._ticker = setTimeout(() => this._tick(), 1000 - (elapsed % 1000) + 10);
    }

    /** Follows the preview's place each frame: shown once it holds still, put away while it moves. */
    _startPreview() {
        if (this._frame)
            return;
        let last = null, still = 0, shown = null;
        const follow = () => {
            this._frame = requestAnimationFrame(follow);
            const box = this._preview.getBoundingClientRect();
            const rect = [box.left, box.top, box.width, box.height].map(Math.round);
            const same = last && rect.every((value, index) => value === last[index]);
            last = rect;
            still = same ? still + 1 : 0;
            // Moving (opening, sliding in): the copy, which ignores the island's shape, waits.
            if (!same && shown) {
                shown = null;
                invoke('call_preview', {rect: null});
            }
            const opaque = getComputedStyle(this.node).opacity === '1';
            if (still >= STILL_FRAMES && opaque && !shown && rect[2] > 0) {
                shown = rect;
                invoke('call_preview', {rect});
            }
            this._preview.classList.toggle('live', Boolean(shown));
        };
        this._frame = requestAnimationFrame(follow);
    }

    _stopPreview() {
        if (!this._frame)
            return;
        cancelAnimationFrame(this._frame);
        this._frame = 0;
        this._preview.classList.remove('live');
        invoke('call_preview', {rect: null});
    }

    /** The clock and the preview only run while the page is on screen. */
    setActive(active) {
        this._active = active;
        this._update();
        if (!active)
            this._stopPreview();
    }
}
