// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// What the island shows and when (gnome-extension/src/controller.js): closed, it shows the time and what is
// going on; rest the pointer on it and it opens, passing over it on the way elsewhere leaves it closed;
// leave it and it closes. The window lets clicks through everywhere but on the island.

import {CompactView} from './compact.js';
import {daemon} from './daemon.js';
import {ExpandedView} from './expanded.js';
import {GlassBackdrop} from './glass.js';
import {Glow, PALETTES, RED, paletteAround} from './glow.js';
import {Island} from './island.js';
import {AppsPage} from './pages/apps.js';
import {AssistantPage} from './pages/assistant.js';
import {CallPage} from './pages/call.js';
import {CalendarPage} from './pages/calendar.js';
import {MusicPage} from './pages/music.js';
import {NotificationsPage} from './pages/notifications.js';
import {RemindersPage} from './pages/reminders.js';
import {ShelfPage} from './pages/shelf.js';
import {TimerPage} from './pages/timer.js';
import {UsagePage} from './pages/usage.js';
import * as Theme from './theme.js';
import {VoiceView} from './voice.js';
import {PeekView, REMINDER_DURATION, SYSTEM_ALERTS, agentAsks, agentEvent, meetingAlert, notificationAlert, pomodoroFinished, systemAlert, toneOf, trackAlert,
    usageAlert} from './peek.js';

const {invoke} = window.__TAURI__.core;
const {listen} = window.__TAURI__.event;

/** Sizes in CSS pixels, as controller.js has them for an island hanging from the screen's top edge; the
 * shoulders a third narrower than GNOME's (10, 22 and 13), the curves into the screen's edge gentler. */
const COMPACT_SHOULDER = 7;
const EXPANDED_RADIUS = 32;
const EXPANDED_SHOULDER = 15;
const PEEK_RADIUS = 28;
const PEEK_SHOULDER = 9;
/** An alert held under the pointer goes this long after the pointer leaves it. */
const ALERT_LEAVE_MS = 1500;
/** An alert shows at least this long; then it stays until a key or the pointer moves, and this long after. */
const ALERT_MIN_MS = 5000, ALERT_SEEN_MS = 3000;
/** Input this soon after an alert came doesn't count as seeing it (the user was already typing). */
const ALERT_GRACE_MS = 1200;
/** How far the pointer may move while it rests before the island opens (for Settings' hover-delay). */
const REST_DISTANCE_PX = 10;
/** Faded away by auto-hide, this much of the island's top brings it back (controller.js REVEAL_EDGE_PX). */
const REVEAL_EDGE_PX = 2;

/** A setting, as the Settings window last set it. */
const setting = key => daemon.state.settings?.[key];
/** Leaving the open island closes it after this long. */
const COLLAPSE_DELAY_MS = 300;
/** The pointer counts as on the open island this far past its edge, so a hand's wobble keeps it open. */
const OPEN_SLACK_PX = 6;

class Controller {
    constructor() {
        const root = document.getElementById('isleIsland');
        this._island = new Island(root);
        this._glow = new Glow(root.querySelector('#glow'), this._island);
        // A view's width changing (a song starting, lyrics turned on) reshapes the island once, on the next frame.
        const reshape = () => {
            if (!this._reshape)
                this._reshape = requestAnimationFrame(() => {
                    this._reshape = 0;
                    this._present();
                });
        };
        this._compact = new CompactView(reshape);
        const calendar = new CalendarPage();
        // An event with a time nears: said in the island, with Join for its meeting link.
        daemon.connect((_key, event) => {
            if (!setting('hidden-pages')?.includes('calendar'))
                this._post(meetingAlert(event, () => invoke('join_meeting', {url: event.link})));
        }, 'eventSoon');
        const notifications = new NotificationsPage();
        notifications.onClose = () => this.collapse();
        const shelf = new ShelfPage();
        shelf.onClose = () => this.collapse();
        this._shelf = shelf;
        const call = new CallPage();
        call.onClose = () => this.collapse();
        // A new call is what the island opens on next, whatever was picked before.
        daemon.connect((_key, state) => {
            if (state?.active && !this._onCall)
                queueMicrotask(() => this._expanded.showPageNamed('call'));
            this._onCall = Boolean(state?.active);
        }, 'call');
        this._pages = [call, new MusicPage(), notifications, new TimerPage(), new RemindersPage(), shelf, calendar, new AppsPage(),
            new UsagePage(), new AssistantPage()];
        this._expanded = new ExpandedView(this._pages, reshape);
        this._peek = new PeekView();
        // Dictating (or the assistant listening): the microphone's wave in the closed island, over any alert.
        this._voice = new VoiceView();
        daemon.connect((_key, state) => this._onVoice(state), 'voice');
        daemon.connect((_key, level) => {
            this._voice.pushLevel(level);
            this._glow.voice(level);
        }, 'voiceLevel');
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
            // Hidden by a full-screen app, it is asked in the terminal, as Isle does.
            if (this._hidden) {
                daemon.call('Agents1', 'Resolve', {id: request.id, answer: 'terminal'});
                return;
            }
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
        daemon.connect((_key, [provider, window, percent, resetsAt]) =>
            this._post(usageAlert(provider, window, percent, resetsAt, setting('usage-critical') ?? 95)), 'usageAlert');
        // Charging, the battery, going offline, Bluetooth: each as Settings says; only a low battery through Do Not Disturb.
        daemon.connect((_key, event) => {
            const alert = systemAlert(event);
            if (!alert || setting(`alert-${SYSTEM_ALERTS[event.kind] ?? 'power'}`) === false)
                return;
            if (daemon.state.notifications?.dnd && event.kind !== 'battery_low')
                return;
            this._post(alert);
        }, 'systemAlert');
        // The assistant: its answer, what went wrong, and the command it asks to run (Run or Deny), in the closed island.
        daemon.connect((_key, state) => this._onAssistant(state), 'assistant');
        daemon.connect((_key, text) => this._post({key: 'assistant-reminder', icon: 'isle-bell', title: text, subtitle: 'Reminder',
            tone: 'warn', duration: REMINDER_DURATION}), 'assistantReminder');
        daemon.connect((_key, page) => {
            this.expand();
            this._expanded.showPageNamed(page);
        }, 'assistantShow');
        daemon.connect(() => this._onNowPlaying(), 'nowPlaying');
        // An app's notification: in the island too, unless Settings or Do Not Disturb say not.
        daemon.connect((_key, note) => {
            if (setting('notification-alerts') !== false && !daemon.state.notifications?.dnd)
                this._post(notificationAlert(note, () => daemon.call('Notifications1', 'Open', {id: note.id})));
        }, 'notificationPosted');
        daemon.connect(() => this._syncFullscreen(), 'fullscreen');
        daemon.connect(() => this._syncAutoHide(), 'windowAtTop');
        daemon.connect(() => {
            this._syncFullscreen();
            this._syncAutoHide(true);
            this._syncTheme();
            this._syncGlow();
        }, 'settings');
        const views = root.querySelector('#views');
        for (const view of [this._compact.node, this._expanded.node, this._peek.node, this._voice.node]) {
            view.style.visibility = 'hidden';
            views.append(view);
        }
        this._presentation = 'compact';
        this._pointer = {x: -1, y: -1};
        this._interactive = false;
        this._typing = false;
        this._timers = {};
        this._island.onFrame = () => this._syncInteractive();
        // A right click on the closed island brings the call to the front.
        this._compact.node.addEventListener('contextmenu', event => {
            event.preventDefault();
            if (this._presentation === 'compact' && daemon.state.call?.active)
                invoke('call_raise');
        });
        // A click opens the closed island, also where Settings say it opens on click.
        this._compact.node.addEventListener('click', () => {
            if (this._presentation === 'compact' && !this._island.autoHidden)
                this.expand();
        });
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
        // Files dragged onto the island: Isle's own drop zone over it says so (dropzone.rs), and Tauri where it can.
        await listen('fileDrag', event => this._onDragDrop(event.payload));
        // The glass's backdrop: a new picture of what is behind (glass.rs), or none to be had.
        await listen('glass-frame', () => this._onGlassFrame());
        await listen('glass-unavailable', () => {
            this._glassFailed = true;
            this._syncGlass();
        });
        await window.__TAURI__.webviewWindow.getCurrentWebviewWindow().onDragDropEvent(event => this._onDragDrop(event.payload));
        this._lastTrack = this._trackOf(daemon.state.nowPlaying);
        this._onCall = Boolean(daemon.state.call?.active);
        // Events due an alert before the page was there to hear of them (Isle starting just before one).
        for (const event of daemon.state.eventsSoon ?? []) {
            if (!setting('hidden-pages')?.includes('calendar'))
                this._post(meetingAlert(event, () => invoke('join_meeting', {url: event.link})));
        }
        this._syncFullscreen();
        this._syncAutoHide();
        this._syncTheme();
        this._present();
    }

    /** The island in Isle's black or in glass (Settings: island-theme, glass-tint); the controls follow the tint in
     * three steps, clear, middle and dark, as on GNOME. */
    _syncTheme() {
        const name = setting('island-theme') ?? 'black', tint = setting('glass-tint') ?? 0;
        const key = `${name}:${tint}`;
        if (key === this._themeKey)
            return;
        this._themeKey = key;
        Theme.setTheme(name, tint);
        const glass = Theme.translucent();
        const root = document.getElementById('isleIsland');
        root.classList.toggle('isle-translucent', glass);
        root.classList.toggle('isle-glass-mid', glass && tint >= 34 && tint < 67);
        root.classList.toggle('isle-glass-dark', glass && tint >= 67);
        this._applyLook();
        this._syncGlass();
    }

    /** The island drawn in the look as it is now: over the glass's backdrop once one has come, else the smoke. */
    _applyLook() {
        this._island.fill = Theme.fill();
        this._island.edge = Theme.edge();
        this._island.sheen = Theme.sheen();
        this._island.tone = Theme.blur();
        this._island.redraw();
    }

    /** Captures what is behind the island while glass shows, and not while it is hidden or black (glass.rs). */
    _syncGlass() {
        let want = Theme.translucent() && !this._hidden && !this._glassFailed;
        if (want && !this._backdrop) {
            try {
                this._backdrop = new GlassBackdrop(document.getElementById('glass'));
            } catch (error) {
                console.warn('no glass backdrop:', error);
                this._glassFailed = true;
                want = false;
            }
        }
        if (want === Boolean(this._glassOn))
            return;
        this._glassOn = want;
        invoke('glass_capture', {on: want}).catch(() => {});
        if (!want) {
            this._backdrop?.clear();
            this._island.backdrop = null;
            Theme.setBackdrop(false);
            this._applyLook();
        }
    }

    /** Fetches the newest picture behind the island and draws the glass over it; one fetch at a time, the last
     * one asked for always fetched. */
    async _onGlassFrame() {
        if (!this._glassOn)
            return;
        if (this._fetchingGlass) {
            this._glassPending = true;
            return;
        }
        this._fetchingGlass = true;
        try {
            do {
                this._glassPending = false;
                const bytes = await invoke('glass_frame');
                const buffer = bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes).buffer;
                const view = new DataView(buffer);
                const width = view.getUint32(0, true), height = view.getUint32(4, true);
                if (!this._glassOn || !width || !height || buffer.byteLength < 8 + width * height * 4)
                    continue;
                this._backdrop.setFrame(width, height, new Uint8Array(buffer, 8, width * height * 4));
                if (this._island.backdrop !== this._backdrop) {
                    Theme.setBackdrop(true);
                    this._island.backdrop = this._backdrop;
                    this._applyLook();
                } else {
                    this._island.drawBackdrop();
                }
            } while (this._glassPending);
        } catch (error) {
            console.warn('glass frame:', error);
        } finally {
            this._fetchingGlass = false;
        }
    }

    // Full screen

    /** Over a full-screen app the island is out of the way: hidden whole, the alerts waiting till it leaves full
     * screen; or, with Settings' alerts over full-screen apps, faded away but for them. */
    _syncFullscreen() {
        const hidden = Boolean(daemon.state.fullscreen) && !setting('alerts-over-fullscreen');
        if (hidden === this._hidden)
            return;
        this._hidden = hidden;
        invoke('set_shown', {shown: !hidden});
        this._syncGlass();
        if (hidden) {
            this._onHidden();
            return;
        }
        // What waited shows now.
        if (this._presentation === 'compact')
            this._showNext();
    }

    /** Hidden by a full-screen app: an AI's asks go back to its terminal, as one asked now would; the open island
     * closes, and the alert on screen waits to be seen. */
    _onHidden() {
        for (const ask of this._asks)
            daemon.call('Agents1', 'Resolve', {id: ask.id, answer: 'terminal'});
        this._asks = [];
        const alert = this._peek.alert;
        if (alert && !alert.actions)
            this._waiting.unshift(alert);
        this._cancel('alert');
        this._peek.alert = null;
        this.collapse();
    }

    /** Whether a full-screen app is in front with alerts shown over it (Settings). */
    get _overFullscreen() {
        return Boolean(daemon.state.fullscreen) && Boolean(setting('alerts-over-fullscreen'));
    }

    // Auto-hide

    /** Whether the closed island hides now ('now'), after Settings' wait ('later'), or shows (false): the rule of
     * visibilityRules.js `autoHide`, at once over a full-screen app, which it shows only for alerts. */
    _autoHideWhen() {
        if (this._presentation !== 'compact')
            return false;
        if (this._overFullscreen)
            return 'now';
        const {x, y} = this._pointer;
        if (this._island.contains(x, y))
            return false;
        const mode = setting('auto-hide') ?? 'off';
        if (mode === 'always' || (mode === 'windows' && daemon.state.windowAtTop))
            return 'later';
        return false;
    }

    /** Hides the closed island as Settings say, unless it is in use; brings it back the moment it is needed again
     * (an alert, the open island, the pointer at its place). */
    _syncAutoHide(restart = false) {
        if (restart)
            this._cancel('auto-hide');
        const when = this._autoHideWhen();
        const band = this._overFullscreen ? 0 : REVEAL_EDGE_PX;
        if (when !== 'later')
            this._cancel('auto-hide');
        if (!when || when === 'now') {
            this._island.setAutoHidden(Boolean(when), band);
            return;
        }
        if (this._island.autoHidden || this._timers['auto-hide'])
            return;
        this._later('auto-hide', (setting('auto-hide-delay') ?? 3) * 1000, () => {
            if (this._autoHideWhen())
                this._island.setAutoHidden(true, band);
        });
    }

    // A new song

    _trackOf(np) {
        return np?.available && np.title ? `${np.title}\u001f${np.artist}` : null;
    }

    /** A new song that starts playing peeks in, if Settings want that. */
    _onNowPlaying() {
        const np = daemon.state.nowPlaying;
        const track = this._trackOf(np);
        const changed = track !== this._lastTrack;
        this._lastTrack = track;
        if (changed && track && np.status === 'Playing' && setting('track-peek') &&
            !setting('hidden-pages')?.includes('music'))
            this._post(trackAlert(np, daemon.art()));
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
        // Dictating: it waits till the words are typed.
        if (this._presentation === 'voice') {
            this._waiting = this._waiting.filter(each => each.key !== alert.key).concat(alert);
            return;
        }
        // Hidden by a full-screen app: it waits.
        if (this._hidden) {
            this._waiting = this._waiting.filter(each => each.key !== alert.key).concat(alert);
            return;
        }
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
        if (setting('edge-light') !== false)
            this._glow.comet(this._redLight ? RED : toneOf(alert));
        this._presentation = 'peek';
        this._cancel('alert');
        // A song change just passes; anything else stays until the user has been seen at the computer.
        this._alertShown = performance.now();
        this._alertNoticed = alert.key === 'track';
        if (!alert.actions) {
            const duration = this._alertNoticed ? alert.duration ?? 2600 : Math.max(alert.duration ?? 0, ALERT_MIN_MS);
            this._alertEnds = performance.now() + duration;
            this._later('alert', duration, () => this._alertTimeUp());
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
    async _alertTimeUp() {
        const {x, y} = this._pointer;
        if (this._island.contains(x, y))
            return;
        const alert = this._peek.alert;
        if (alert && !this._alertNoticed) {
            const idle = await invoke('idle_ms').catch(() => 0);
            if (this._peek.alert !== alert || this._timers.alert)
                return;
            // A key or the pointer moved since it came (a moment after, so typing already under way doesn't count).
            if (idle < performance.now() - this._alertShown - ALERT_GRACE_MS)
                this._noticed();
            else
                this._later('alert', 1000, () => this._alertTimeUp());
            return;
        }
        this._dismissAlert();
    }

    /** The user has seen the alert: it stays a few seconds more, and the edge stops calling. */
    _noticed() {
        if (this._alertNoticed)
            return;
        this._alertNoticed = true;
        this._syncGlow();
        if (!this._peek.alert?.actions) {
            this._alertEnds = performance.now() + ALERT_SEEN_MS;
            this._later('alert', ALERT_SEEN_MS, () => this._alertTimeUp());
        }
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

    /** The assistant answered, failed, or asks to run a command; the open island's Assistant page shows the same. */
    _onAssistant(state) {
        this._syncGlow();
        if (!state)
            return;
        const cardId = state.card && !state.card.running ? `assistant-card:${state.card.id}` : null;
        // A card answered or gone: its ask goes too.
        for (const ask of this._asks.filter(each => each.id?.startsWith?.('assistant-card:') && each.id !== cardId)) {
            this._asks = this._asks.filter(each => each !== ask);
            if (this._peek.alert?.id === ask.id)
                this._dismissAlert();
        }
        if (cardId && !this._asks.some(ask => ask.id === cardId) && this._lastCard !== cardId) {
            this._lastCard = cardId;
            if (this._presentation === 'expanded')
                return;
            const {card} = state;
            const answer = run => {
                invoke('assistant_answer', {id: card.id, run});
                this._asks = this._asks.filter(ask => ask.id !== cardId);
            };
            this._asks.push({
                key: cardId,
                id: cardId,
                icon: 'isle-terminal-symbolic',
                title: card.why,
                subtitle: card.detail || 'The assistant asks to run this',
                code: card.command,
                tone: 'warn',
                actions: [
                    {label: 'Deny', answer: 'deny', callback: () => answer(false)},
                    {label: card.root ? 'Run as admin' : 'Run', answer: 'allow', done: 'Running', primary: true, callback: () => answer(true)},
                ],
                sticky: true,
                duration: 600000,
            });
            if (this._peek.alert?.actions === undefined)
                this._showNext();
            return;
        }
        if (!['reply', 'error'].includes(state.phase) || state.turn === this._lastTurn)
            return;
        this._lastTurn = state.turn;
        if (state.phase === 'error') {
            this._post({key: 'assistant', icon: 'isle-warning-symbolic', title: 'Assistant', subtitle: state.error,
                subtitleLines: 2, tone: 'bad', duration: REMINDER_DURATION, page: 'assistant'});
            return;
        }
        const reply = String(state.reply ?? '').replace(/\*\*|`/g, '').replace(/\s+/g, ' ').trim();
        if (!reply)
            return;
        const words = reply.split(' ').length;
        this._post({key: 'assistant', icon: 'isle-sparkle-symbolic', title: reply, titleLines: 2,
            subtitle: state.heard ? `“${state.heard}”` : 'Assistant', tone: 'claude',
            duration: Math.min(14000, 3500 + words * 260), page: 'assistant'});
    }

    /** Voice shows (dictating, the words typed) or goes; the open island stays open, dictation working under it. */
    _onVoice(state) {
        this._voice.set(state);
        if (state) {
            if (this._presentation === 'expanded')
                return;
            if (this._presentation === 'peek') {
                const alert = this._peek.alert;
                // Shown again after, unless it asks (that one comes back from the asks).
                if (alert && !alert.actions)
                    this._waiting.unshift(alert);
                this._cancel('alert');
                this._peek.alert = null;
            }
            this._presentation = 'voice';
            this._present();
        } else if (this._presentation === 'voice') {
            this._presentation = 'compact';
            this._showNext();
            if (this._presentation === 'compact')
                this._present();
        }
        // Words typed or a request heard: the light flares once as it settles.
        if (state?.phase === 'done' && this._voicePhase !== 'done')
            this._glow.pulse();
        this._voicePhase = state?.phase;
        this._syncGlow();
    }

    /** The edge light for what the island shows: the assistant's colours while it listens or works, dictation's
     * green while the microphone is on, and an alert that waits to be seen or answered beating like a heart
     * in its colour. Settings' Edge light off puts it out. */
    get _redLight() {
        return (setting('edge-light-color') ?? 'red') === 'red';
    }

    _syncGlow() {
        if (!this._glow)
            return;
        this._glow.alwaysMove = setting('edge-light-motion') !== false;
        if (setting('edge-light') === false) {
            this._glow.clear();
            return;
        }
        const voice = this._presentation === 'voice' ? this._voice.state : null;
        // Red throughout (Settings' default), or each in its own colours.
        const red = this._redLight;
        const assistant = daemon.state.assistant ?? {};
        const busy = ['listening', 'transcribing', 'thinking', 'acting'].includes(assistant.phase);
        if (voice && voice.phase !== 'error') {
            const working = voice.phase === 'working';
            const breathing = voice.phase === 'listening';
            const level = breathing ? 0.3 : 0.4;
            // Green whenever the voice is on, dictating or giving the assistant a command.
            this._glow.aura(PALETTES.voice, {level, working, breathing});
        } else if (this._presentation === 'expanded' && busy) {
            this._glow.aura(red ? PALETTES.red : PALETTES.assistant, {level: 0.3, working: assistant.phase !== 'listening'});
        } else if (this._presentation === 'peek' && (this._peek.alert?.actions || this._peek.alert && !this._alertNoticed)) {
            this._glow.aura(red ? PALETTES.red : paletteAround(toneOf(this._peek.alert)), {level: 0.35, heartbeat: true});
        } else {
            this._glow.aura(null);
        }
    }

    _onPointer({x, y}) {
        this._pointer = {x, y};
        this._syncAutoHide();
        if (this._presentation === 'voice') {
            this._syncInteractive();
            return;
        }
        if (this._presentation === 'peek') {
            const over = this._island.contains(x, y);
            this._syncInteractive();
            if (this._peek.alert?.actions)
                return;
            if (over) {
                this._cancel('alert');
                this._noticed();
            } else if (!this._timers.alert)
                this._later('alert', Math.max(ALERT_LEAVE_MS, this._alertEnds - performance.now()), () => this._alertTimeUp());
            return;
        }
        const open = this._presentation === 'expanded';
        const over = this._island.contains(x, y, open ? OPEN_SLACK_PX : 0);
        this._syncInteractive();
        if (open) {
            if (over || this._typing || this._dragging)
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
        // Faded away, the pointer at its place brings it back; it opens only on a click (Settings: open on click).
        if (this._island.autoHidden || setting('open-on') === 'click')
            return;
        // Resting: it opens once the pointer has stayed within a few pixels for the hover delay.
        if (!this._rest || Math.hypot(x - this._rest.x, y - this._rest.y) > REST_DISTANCE_PX) {
            this._rest = {x, y};
            this._later('expand', setting('hover-delay') ?? 120, () => {
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
        const interactive = this._typing || this._dragging || this._island.contains(x, y, slack);
        if (interactive !== this._interactive) {
            this._interactive = interactive;
            invoke('set_interactive', {interactive});
        }
        // Where files dragged in are taken: the island as it is heading to be.
        const r = this._island.targetRect;
        const area = [r.x, r.y, r.width, r.height].map(Math.round);
        if (area.join() !== this._dropArea?.join()) {
            this._dropArea = area;
            invoke('set_drop_area', {rect: area});
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

    /** Files dragged onto the island (Windows hands the drop to the window once the pointer is over the island):
     * it opens on the Shelf's Files tab to drop them on, as Isle does on GNOME. */
    _onDragDrop({type, paths, position}) {
        const ratio = window.devicePixelRatio || 1;
        const [x, y] = position ? [position.x / ratio, position.y / ratio] : [-1, -1];
        const shelf = this._shelf;
        if (type === 'enter') {
            // Left out in Settings: no shelf to drop on.
            if (setting('hidden-pages')?.includes('shelf') || !paths?.length)
                return;
            this._dragging = true;
            shelf.showTab('files');
            this.expand();
            this._expanded.showPageNamed('shelf');
            shelf.dropping = true;
            shelf.dragOver(paths, x, y);
        } else if (!this._dragging) {
            return;
        } else if (type === 'over') {
            this._cancel('collapse');
            shelf.dragOver(null, x, y);
        } else if (type === 'drop') {
            this._dragging = false;
            shelf.drop(paths, x, y);
        } else {
            this._dragging = false;
            shelf.dropping = false;
        }
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
        } else if (this._presentation === 'voice') {
            size = {width: this._voice.preferredWidth, height: this._voice.height,
                radius: this._voice.height / 2, shoulder: PEEK_SHOULDER};
            view = this._voice;
        } else {
            size = {width: this._compact.preferredWidth, height: this._compact.height,
                radius: this._compact.radius, shoulder: COMPACT_SHOULDER};
            view = this._compact;
        }
        size.width = 2 * Math.round(size.width / 2);
        this._island.morph(size, view.node);
        this._syncAutoHide();
        const open = this._presentation === 'expanded';
        if (open !== this._open) {
            this._open = open;
            this._expanded.setActive(open);
        }
        this._syncGlow();
    }
}

new Controller().start();
