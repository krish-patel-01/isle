// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// What the island shows and when (gnome-extension/src/controller.js): closed, it shows the time and what is
// going on; rest the pointer on it and it opens, passing over it on the way elsewhere leaves it closed;
// leave it and it closes. The window lets clicks through everywhere but on the island.

import {CompactView} from './compact.js';
import {daemon} from './daemon.js';
import {ExpandedView} from './expanded.js';
import {Island} from './island.js';
import {MusicPage} from './pages/music.js';
import {RemindersPage} from './pages/reminders.js';
import {TimerPage} from './pages/timer.js';
import {UsagePage} from './pages/usage.js';
import {PeekView, agentAsks, agentEvent, pomodoroFinished} from './peek.js';

const {invoke} = window.__TAURI__.core;
const {listen} = window.__TAURI__.event;

/** Sizes in CSS pixels, as controller.js has them for an island hanging from the screen's top edge. */
const COMPACT_SHOULDER = 10;
const EXPANDED_RADIUS = 32;
const EXPANDED_SHOULDER = 22;
const PEEK_RADIUS = 28;
const PEEK_SHOULDER = 13;
/** An alert held under the pointer goes this long after the pointer leaves it. */
const ALERT_LEAVE_MS = 1500;
/** How long the pointer rests before the island opens (Settings: hover-delay), and how far it may move meanwhile. */
const HOVER_DELAY_MS = 120;
const REST_DISTANCE_PX = 10;
/** Leaving the open island closes it after this long. */
const COLLAPSE_DELAY_MS = 300;
/** The pointer counts as on the open island this far past its edge, so a hand's wobble keeps it open. */
const OPEN_SLACK_PX = 6;

class Controller {
    constructor() {
        const root = document.getElementById('isleIsland');
        this._island = new Island(root);
        // A view's width changing (a song starting, lyrics turned on) reshapes the island once, on the next frame.
        const reshape = () => {
            if (!this._reshape)
                this._reshape = requestAnimationFrame(() => {
                    this._reshape = 0;
                    this._present();
                });
        };
        this._compact = new CompactView(reshape);
        this._pages = [new MusicPage(), new TimerPage(), new RemindersPage(), new UsagePage()];
        this._expanded = new ExpandedView(this._pages, reshape);
        this._peek = new PeekView();
        this._peek.onOpen = alert => {
            this._dismissAlert(false);
            if (alert.page)
                this._expanded.showPageNamed(alert.page);
            this.expand();
        };
        this._peek.onDone = () => this._dismissAlert();
        /** Permissions waiting for an answer, oldest first; and other alerts waiting their turn. */
        this._asks = [];
        this._waiting = [];
        daemon.connect((_key, [finished, next]) => {
            const state = daemon.state.pomodoro;
            this._post(pomodoroFinished(finished, next, state, () => daemon.call('pomodoro', 'Start')));
        }, 'phaseFinished');
        daemon.connect((_key, request) => {
            const answer = choice => {
                daemon.call('Agents1', 'Resolve', {id: request.id, answer: choice});
                this._asks = this._asks.filter(ask => ask.id !== request.id);
            };
            this._asks.push(agentAsks(request, answer));
            // Asking comes first, even over the open island.
            if (this._peek.alert?.actions === undefined)
                this._showNext();
        }, 'agentRequest');
        daemon.connect((_key, id) => {
            this._asks = this._asks.filter(ask => ask.id !== id);
            if (this._peek.alert?.id === id)
                this._dismissAlert();
        }, 'agentGone');
        daemon.connect((_key, event) => this._post(agentEvent(event)), 'agentEvent');
        const views = root.querySelector('#views');
        for (const view of [this._compact.node, this._expanded.node, this._peek.node]) {
            view.style.visibility = 'hidden';
            views.append(view);
        }
        this._presentation = 'compact';
        this._pointer = {x: -1, y: -1};
        this._interactive = false;
        this._typing = false;
        this._timers = {};
        this._island.onFrame = () => this._syncInteractive();
        window.isleKeyboard = wanted => this._setTyping(wanted);
        // Keys typed while the island holds the keyboard: Escape closes it.
        window.addEventListener('keydown', event => {
            if (event.key === 'Escape' && !(event.target instanceof HTMLInputElement))
                this.collapse();
        });
        this._present();
    }

    async start() {
        await daemon.start();
        await listen('pointer', event => this._onPointer(event.payload));
        this._present();
    }

    _later(name, ms, fn) {
        clearTimeout(this._timers[name]);
        this._timers[name] = setTimeout(() => {
            delete this._timers[name];
            fn();
        }, ms);
    }

    _cancel(name) {
        clearTimeout(this._timers[name]);
        delete this._timers[name];
    }

    /** Shows `alert` in the closed island for its duration; one that asks waits its turn behind it, and the
     * open island keeps open (its page says the same). */
    _post(alert) {
        if (this._presentation === 'expanded')
            return;
        if (this._peek.alert && (this._peek.alert.actions || this._asks.length)) {
            this._waiting = this._waiting.filter(each => each.key !== alert.key).concat(alert);
            return;
        }
        this._show(alert);
    }

    _show(alert) {
        if (this._presentation === 'expanded') {
            document.activeElement?.blur?.();
            if (this._typing) {
                this._typing = false;
                invoke('set_keyboard', {wanted: false});
            }
        }
        this._peek.show(alert);
        this._presentation = 'peek';
        this._cancel('alert');
        if (!alert.actions) {
            this._alertEnds = performance.now() + (alert.duration ?? 2600);
            this._later('alert', alert.duration ?? 2600, () => this._alertTimeUp());
        }
        this._present();
    }

    /** The next permission waiting, else the next alert, else the closed island. */
    _showNext() {
        const next = this._asks[0] ?? this._waiting.shift();
        if (next)
            this._show(next);
        else if (this._presentation === 'peek') {
            this._presentation = 'compact';
            this._present();
        }
    }

    /** Its time is up: it goes, unless the pointer holds it (then it goes a moment after it leaves). */
    _alertTimeUp() {
        const {x, y} = this._pointer;
        if (this._island.contains(x, y))
            return;
        this._dismissAlert();
    }

    _dismissAlert(present = true) {
        this._cancel('alert');
        this._peek.alert = null;
        if (this._presentation === 'peek') {
            if (present)
                this._showNext();
            else
                this._presentation = 'compact';
        }
    }

    _onPointer({x, y}) {
        this._pointer = {x, y};
        if (this._presentation === 'peek') {
            const over = this._island.contains(x, y);
            this._syncInteractive();
            if (this._peek.alert?.actions)
                return;
            if (over)
                this._cancel('alert');
            else if (!this._timers.alert)
                this._later('alert', Math.max(ALERT_LEAVE_MS, this._alertEnds - performance.now()), () => this._alertTimeUp());
            return;
        }
        const open = this._presentation === 'expanded';
        const over = this._island.contains(x, y, open ? OPEN_SLACK_PX : 0);
        this._syncInteractive();
        if (open) {
            if (over || this._typing)
                this._cancel('collapse');
            else if (!this._timers.collapse)
                this._later('collapse', COLLAPSE_DELAY_MS, () => this.collapse());
            return;
        }
        if (!over) {
            this._cancel('expand');
            this._rest = null;
            return;
        }
        // Resting: it opens once the pointer has stayed within a few pixels for the hover delay.
        if (!this._rest || Math.hypot(x - this._rest.x, y - this._rest.y) > REST_DISTANCE_PX) {
            this._rest = {x, y};
            this._later('expand', HOVER_DELAY_MS, () => {
                const {x: nowX, y: nowY} = this._pointer;
                if (this._island.contains(nowX, nowY) && Math.hypot(nowX - this._rest.x, nowY - this._rest.y) <= REST_DISTANCE_PX)
                    this.expand();
            });
        }
    }

    /** Clicks reach the window only while the pointer is on the island (or the island is being typed in). */
    _syncInteractive() {
        const {x, y} = this._pointer;
        const slack = this._presentation === 'expanded' ? OPEN_SLACK_PX : 0;
        const interactive = this._typing || this._island.contains(x, y, slack);
        if (interactive !== this._interactive) {
            this._interactive = interactive;
            invoke('set_interactive', {interactive});
        }
    }

    _setTyping(wanted) {
        this._typing = wanted;
        if (wanted)
            invoke('set_keyboard', {wanted: true});
        this._syncInteractive();
        // Done typing with the pointer gone: close, as leaving would have.
        if (!wanted && this._presentation === 'expanded') {
            const {x, y} = this._pointer;
            if (!this._island.contains(x, y, OPEN_SLACK_PX))
                this._later('collapse', COLLAPSE_DELAY_MS, () => this.collapse());
        }
    }

    expand() {
        if (this._presentation === 'expanded')
            return;
        this._presentation = 'expanded';
        this._present();
    }

    collapse() {
        if (this._presentation === 'compact')
            return;
        if (document.activeElement instanceof HTMLElement)
            document.activeElement.blur();
        if (this._typing) {
            this._typing = false;
            invoke('set_keyboard', {wanted: false});
        }
        this._presentation = 'compact';
        this._present();
    }

    _present() {
        let size, view;
        if (this._presentation === 'expanded') {
            size = {width: this._expanded.preferredWidth, height: this._expanded.height,
                radius: EXPANDED_RADIUS, shoulder: EXPANDED_SHOULDER};
            view = this._expanded;
        } else if (this._presentation === 'peek') {
            size = {width: this._peek.preferredWidth, height: this._peek.height,
                radius: PEEK_RADIUS, shoulder: PEEK_SHOULDER};
            view = this._peek;
        } else {
            size = {width: this._compact.preferredWidth, height: this._compact.height,
                radius: this._compact.radius, shoulder: COMPACT_SHOULDER};
            view = this._compact;
        }
        size.width = 2 * Math.round(size.width / 2);
        this._island.morph(size, view.node);
        const open = this._presentation === 'expanded';
        if (open !== this._open) {
            this._open = open;
            this._expanded.setActive(open);
        }
    }
}

new Controller().start();
