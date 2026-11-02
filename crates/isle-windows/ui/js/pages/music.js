// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The Music page (gnome-extension/src/pages/media.js and lyrics.js): the cover, title, artist, scrubber and
// controls; or, with lyrics on, a narrower player beside the lines, the one being sung large and bright.

import {daemon} from '../daemon.js';
import {Art, Bars, Slider, el, formatTime, hasTrack, icon, iconButton, positionNow, setButtonIcon} from '../widgets.js';

const ART_SIZE = 104;
const LYRICS_WIDTH = 720;
const PROGRESS_REFRESH_MS = 250;
const PAUSED_BARS_OPACITY = 110 / 255;
/** The sung line sits this far down the lyrics view. */
const ANCHOR = 0.36;
/** After a scroll by hand, the view follows the song again after this long. */
const MANUAL_SCROLL_HOLD_MS = 4000;
const CURRENT_OPACITY = 1;
const COMING_OPACITY = [150 / 255, 115 / 255, 90 / 255];
const SUNG_OPACITY = 75 / 255;
const RESTING_SCALE = 0.93;
/** A gap in the singing this long shows breathing dots. */
const PAUSE_MS = 6000;
const MODE_KEY = 'isle.music.mode';

const STATES = {
    loading: {dots: true, title: 'Looking for lyrics'},
    not_found: {icon: 'isle-lyrics', title: 'No lyrics for this song', hint: 'Lyrics come from LRCLIB'},
    instrumental: {icon: 'isle-music', title: 'Instrumental', hint: 'Nothing to sing along to'},
    off: {icon: 'isle-lyrics', title: 'Lyrics are off', hint: 'Turn them on in Isle’s settings'},
    error: {icon: 'isle-lyrics', title: 'Can’t reach the lyrics', hint: 'Check your connection · trying again soon'},
    unavailable: {icon: 'isle-lyrics', title: 'Lyrics are busy right now', hint: 'Trying again in a moment'},
    none: {icon: 'isle-lyrics', title: 'No lyrics for this song', hint: 'Lyrics come from LRCLIB'},
};

/** One copy of the player's parts (title, scrubber, controls): the player and the lyrics' side each have one. */
class PlayerParts {
    constructor(page, {artSize, artRadius, titleClass, artistClass}) {
        this.art = new Art({size: artSize, radius: artRadius});
        this.art.node.addEventListener('click', () => daemon.call('media', 'Raise'));
        this.title = el('div', titleClass);
        this.artist = el('div', artistClass);
        this.texts = el('div', titleClass === 'isle-track-title' ? 'isle-player-texts' : 'isle-side-texts', [this.title, this.artist]);
        this.elapsed = el('span', 'isle-time', ['0:00']);
        this.remaining = el('span', 'isle-time isle-time-end', ['−0:00']);
        this.slider = new Slider(value => {
            const np = daemon.state.nowPlaying;
            if (np?.can_seek)
                daemon.call('media', 'SetPosition', Math.round(value * np.length_us));
        });
        this.progress = el('div', 'isle-player-progress', [this.elapsed, this.slider.node, this.remaining]);
        this.play = iconButton('isle-play-symbolic', 'isle-button-flat isle-play', () => daemon.call('media', 'PlayPause'), {size: 24, label: 'Play or pause'});
        this.previous = iconButton('isle-backward-symbolic', 'isle-button-flat', () => daemon.call('media', 'Previous'), {size: 20, label: 'Previous'});
        this.next = iconButton('isle-forward-symbolic', 'isle-button-flat', () => daemon.call('media', 'Next'), {size: 20, label: 'Next'});
        this.transport = el('div', 'isle-transport', [this.previous, this.play, this.next]);
        this._page = page;
    }

    update(np) {
        this.title.textContent = np?.title ?? '';
        this.artist.textContent = [np?.artist, np?.album].filter(Boolean).join(' — ') || np?.identity || '';
        setButtonIcon(this.play, np?.status === 'Playing' ? 'isle-pause-symbolic' : 'isle-play-symbolic');
        this.previous.disabled = !np?.can_go_previous;
        this.next.disabled = !np?.can_go_next;
        this.slider.enabled = Boolean(np?.can_seek);
    }

    tick(np) {
        const position = positionNow(np) / 1000;
        const length = (np?.length_us ?? 0) / 1000;
        if (!this.slider.dragging)
            this.slider.value = length > 0 ? position / length : 0;
        const shown = this.slider.dragging ? this.slider.value * length : position;
        this.elapsed.textContent = formatTime(shown);
        this.remaining.textContent = `−${formatTime(Math.max(0, length - shown))}`;
    }
}

export class MusicPage {
    constructor() {
        this.name = 'music';
        this.node = el('div', 'isle-page isle-media');
        try {
            this._mode = localStorage.getItem(MODE_KEY) === 'lyrics' ? 'lyrics' : 'player';
        } catch {
            this._mode = 'player';
        }
        this._active = false;
        this._artKey = null;
        this._userScrollAt = 0;
        this._current = -2;

        this._player = this._buildPlayer();
        this._lyricsLayout = this._buildLyricsLayout();
        this._empty = el('div', 'col isle-empty', [
            icon('isle-music', 28, 'isle-empty-icon'),
            el('div', 'isle-empty-title', ['Nothing playing']),
            el('div', 'isle-empty-hint', ['Play something in Spotify or on YouTube']),
        ]);
        this._empty.style.width = '100%';
        this.node.append(this._player, this._lyricsLayout, this._empty);

        daemon.connect(key => {
            if (key === 'nowPlaying')
                this._update();
            else if (key === 'lyrics')
                this._buildLines();
        });
        this._update();
        this._buildLines();
    }

    get preferredWidth() {
        return this._shown('lyrics') ? LYRICS_WIDTH : null;
    }

    _shown(mode) {
        return this._mode === mode && hasTrack(daemon.state.nowPlaying);
    }

    _modeButton(on) {
        const button = iconButton(on ? 'isle-lyrics-fill' : 'isle-lyrics', `isle-button-flat isle-mode-toggle${on ? ' on' : ''}`,
            () => this._setMode(on ? 'player' : 'lyrics'), {size: 16, label: on ? 'Hide lyrics' : 'Show lyrics'});
        return button;
    }

    _setMode(mode) {
        this._mode = mode;
        try {
            localStorage.setItem(MODE_KEY, mode);
        } catch {}
        this._update();
        this._current = -2;
        this._syncLyrics(true);
        this.onWidth?.();
    }

    _buildPlayer() {
        this._main = new PlayerParts(this, {artSize: ART_SIZE, artRadius: 16, titleClass: 'isle-track-title', artistClass: 'isle-track-subtitle'});
        const parts = this._main;
        const badge = el('div', 'isle-app-badge');
        parts.art.node.append(badge);
        const modes = el('div', 'isle-player-modes', [this._modeButton(false)]);
        const head = el('div', 'isle-player-head', [parts.texts, modes]);
        const column = el('div', 'isle-player-column', [head, parts.progress, parts.transport]);
        return el('div', 'isle-player', [parts.art.node, column]);
    }

    _buildLyricsLayout() {
        this._side = new PlayerParts(this, {artSize: 64, artRadius: 12, titleClass: 'isle-side-title', artistClass: 'isle-side-artist'});
        const parts = this._side;
        this._sideBars = new Bars({height: 11});
        this._sideBars.node.classList.add('isle-side-bars');
        parts.texts.append(this._sideBars.node);
        const top = el('div', 'isle-player-top', [parts.art.node, parts.texts]);
        parts.transport.classList.add('grow');
        const controls = el('div', 'isle-lyrics-controls', [this._modeButton(true), parts.transport]);
        // The output button sits at the right in Isle; volume and output are Windows' own here.
        const spacer = el('span');
        spacer.style.width = '28px';
        controls.append(spacer);
        const side = el('div', 'isle-lyrics-side', [top, parts.progress, controls]);

        this._column = el('div', 'isle-lyrics-column');
        this._lyricsScroll = el('div', 'isle-lyrics-scroll', [this._column]);
        this._lyricsScroll.addEventListener('wheel', event => {
            this._userScrollAt = performance.now();
            this._manualOffset = (this._manualOffset ?? this._offset ?? 0) - event.deltaY;
            this._column.style.transform = `translateY(${this._manualOffset}px)`;
        }, {passive: true});
        return el('div', 'isle-lyrics-layout', [side, this._lyricsScroll]);
    }

    /** Builds a line per lyric, or the state when there are none. */
    _buildLines() {
        const lyrics = daemon.state.lyrics;
        this._lines = [];
        this._column.replaceChildren();
        this._current = -2;
        const status = lyrics?.status ?? 'none';
        if (status !== 'synced' && status !== 'plain') {
            const state = STATES[status] ?? STATES.none;
            const box = el('div', 'isle-lyrics-state');
            if (state.dots)
                box.append(el('div', 'isle-lyric-dots isle-lyrics-state-dots', [0, 1, 2].map(() => el('div', 'isle-lyric-dot'))));
            else
                box.append(icon(state.icon, 22, 'isle-lyrics-state-icon'));
            box.append(el('div', 'isle-lyrics-state-title', [state.title]));
            if (state.hint)
                box.append(el('div', 'isle-lyrics-state-hint', [state.hint]));
            this._column.replaceChildren(box);
            this._column.style.transform = '';
            this._column.style.height = '100%';
            return;
        }
        this._column.style.height = '';
        const synced = status === 'synced';
        const lines = lyrics.lines ?? [];
        lines.forEach((line, index) => {
            const next = lines[index + 1];
            // A long gap after a line: breathing dots in it.
            if (synced && index === 0 && line.time_ms >= PAUSE_MS)
                this._addPause(0, line.time_ms);
            const node = el('div', `isle-lyric-line${synced ? '' : ' plain'}`, [line.text]);
            if (synced)
                node.addEventListener('click', () => daemon.call('media', 'SetPosition', line.time_ms * 1000));
            this._column.append(node);
            this._lines.push({node, time: line.time_ms});
            if (synced && !line.text.trim() && next && next.time_ms - line.time_ms >= PAUSE_MS) {
                node.remove();
                this._lines.pop();
                this._addPause(line.time_ms, next.time_ms);
            }
        });
        this._column.append(el('div', 'isle-lyrics-end'));
        this._syncLyrics(true);
    }

    _addPause(start, end) {
        const node = el('div', 'isle-lyric-dots', [0, 1, 2].map(() => el('div', 'isle-lyric-dot')));
        this._column.append(node);
        this._lines.push({node, time: start, pause: true, end});
    }

    /** Lights the line being sung and scrolls it to the reading line. */
    _syncLyrics(force = false) {
        if (!this._shown('lyrics') || !this._lines?.length || daemon.state.lyrics?.status !== 'synced')
            return;
        const np = daemon.state.nowPlaying;
        const now = positionNow(np) / 1000 + 150;
        let current = -1;
        for (let i = 0; i < this._lines.length; i++) {
            if (this._lines[i].time <= now)
                current = i;
            else
                break;
        }
        if (current === this._current && !force)
            return;
        this._current = current;
        this._lines.forEach(({node, pause}, i) => {
            const distance = i - current;
            const opacity = distance === 0 ? CURRENT_OPACITY : distance < 0 ? SUNG_OPACITY : COMING_OPACITY[Math.min(distance - 1, 2)];
            node.style.opacity = opacity;
            node.style.transform = pause || distance === 0 ? '' : `scale(${RESTING_SCALE})`;
            for (const dot of node.querySelectorAll?.('.isle-lyric-dot') ?? [])
                dot.style.animationPlayState = distance === 0 && np?.status === 'Playing' ? 'running' : 'paused';
        });
        if (performance.now() - this._userScrollAt < MANUAL_SCROLL_HOLD_MS)
            return;
        this._manualOffset = null;
        const target = this._lines[Math.max(0, current)].node;
        const anchor = this._lyricsScroll.clientHeight * ANCHOR;
        this._offset = Math.round(anchor - target.offsetTop - target.offsetHeight / 2);
        if (current < 0)
            this._offset = Math.min(this._offset, Math.round(anchor * 0.5));
        this._column.style.transform = `translateY(${this._offset}px)`;
    }

    async _syncArt(np) {
        if (!np?.art_key || np.art_key === this._artKey)
            return;
        this._artKey = np.art_key;
        this._main.art.url = this._side.art.url = null;
        const url = await daemon.art();
        if (this._artKey === np.art_key)
            this._main.art.url = this._side.art.url = url;
    }

    _update() {
        const np = daemon.state.nowPlaying;
        const track = hasTrack(np);
        const lyrics = track && this._mode === 'lyrics';
        this._player.style.display = track && !lyrics ? '' : 'none';
        this._lyricsLayout.style.display = lyrics ? '' : 'none';
        this._empty.style.display = track ? 'none' : '';
        if (track) {
            this._main.update(np);
            this._side.update(np);
            this._syncArt(np);
        }
        const playing = np?.status === 'Playing';
        this._sideBars.playing = playing && this._active && lyrics;
        this._sideBars.opacity = playing ? 1 : PAUSED_BARS_OPACITY;
        this._tick();
        this._syncTicker();
        this.onWidth?.();
    }

    _tick() {
        const np = daemon.state.nowPlaying;
        if (!hasTrack(np))
            return;
        this._main.tick(np);
        this._side.tick(np);
        this._syncLyrics();
    }

    _syncTicker() {
        clearInterval(this._ticker);
        if (this._active && daemon.state.nowPlaying?.status === 'Playing')
            this._ticker = setInterval(() => this._tick(), PROGRESS_REFRESH_MS);
    }

    setActive(active) {
        this._active = active;
        this._update();
        if (active)
            requestAnimationFrame(() => this._syncLyrics(true));
    }
}
