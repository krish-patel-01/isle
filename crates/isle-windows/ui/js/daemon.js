// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The island's view of the service (gnome-extension/src/daemon.js): the same state keys and calls, served
// by the Rust side of this app through Tauri instead of by isled over D-Bus.

const {invoke} = window.__TAURI__.core;
const {listen} = window.__TAURI__.event;

/** Interface names as on D-Bus, by the short names the pages use. */
const INTERFACES = {media: 'Media1', pomodoro: 'Pomodoro1', todo: 'Todo1'};

class Daemon {
    constructor() {
        this.state = {nowPlaying: null, lyrics: null, pomodoro: null, pomodoroConfig: null, todo: [], usage: null};
        this._handlers = new Map();
    }

    /** Calls `fn(key)` whenever a state key changes; `key` limits it to one. */
    connect(fn, key = null) {
        const list = this._handlers.get(key) ?? [];
        list.push(fn);
        this._handlers.set(key, list);
    }

    _changed(key, value) {
        this.state[key] = value;
        for (const fn of this._handlers.get(key) ?? [])
            fn(key, value);
        for (const fn of this._handlers.get(null) ?? [])
            fn(key, value);
    }

    /** A daemon method: `call('media', 'PlayPause')`, `call('todo', 'Add', {title})`. */
    call(name, method, arg = null) {
        return invoke('call', {interface: INTERFACES[name] ?? name, method, arg});
    }

    /** The current track's cover as a data: URL, or null. Players (Spotify) can attach it a while after the song
     * starts, so a miss is asked again a few times; one request per track is shared by every view. */
    art() {
        const key = this.state.nowPlaying?.art_key;
        if (this._artKey !== key) {
            this._artKey = key;
            this._art = (async () => {
                for (const wait of [0, 1500, 4000, 8000, 15000]) {
                    if (wait)
                        await new Promise(resolve => setTimeout(resolve, wait));
                    if (this._artKey !== key)
                        return null;
                    const url = await invoke('media_art');
                    if (url)
                        return url;
                }
                return null;
            })();
        }
        return this._art;
    }

    async start() {
        for (const key of Object.keys(this.state))
            await listen(key, event => this._changed(key, event.payload));
        // Signals: something happened, with no state kept (a phase ran out, an AI asks, is done, stops asking).
        for (const signal of ['phaseFinished', 'agentRequest', 'agentGone', 'agentEvent']) {
            await listen(signal, event => {
                for (const fn of this._handlers.get(signal) ?? [])
                    fn(signal, event.payload);
            });
        }
        const snapshot = await invoke('snapshot');
        for (const [key, value] of Object.entries(snapshot))
            this._changed(key, value);
    }
}

export const daemon = new Daemon();
