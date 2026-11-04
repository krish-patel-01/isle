// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// Header and pager of the open island (gnome-extension/src/expanded.js): tabs on the left, the open one lit
// with its name; on the right the music bars, the focus ring, the date and the time. Pages slide sideways.

import {timerColor} from './compact.js';
import {daemon} from './daemon.js';
import {Bars, Ring, clockParts, el, icon, iconButton, longDate, setIcon, showsClosed, watchClock} from './widgets.js';

const {invoke} = window.__TAURI__.core;

export const EXPANDED_WIDTH = 600;
export const HEADER_HEIGHT = 32;
export const PAGE_HEIGHT = 150;
export const BOTTOM_PADDING = 20;

const TABS = {
    call: {icon: 'isle-phone', label: 'Call'},
    music: {icon: 'isle-music', label: 'Music'},
    notifications: {icon: 'isle-bell', label: 'Notifications'},
    focus: {icon: 'isle-timer', label: 'Timer'},
    reminders: {icon: 'isle-tasks', label: 'Reminders'},
    calendar: {icon: 'isle-calendar', label: 'Calendar'},
    apps: {icon: 'isle-apps', label: 'Apps'},
    usage: {icon: 'isle-gauge', label: 'Usage'},
};
const PAUSED_BARS_OPACITY = 110 / 255;
const SLIDE_DISTANCE = 48;
const SLIDE_DURATION = 260;
/** Wheel travel over the header that turns a page, and the least time between pages a spin turns. */
const WHEEL_STEP = 100;
const WHEEL_REPEAT_MS = 200;
const PAGE_KEY = 'isle.page';
/** The settings icon's, as bright as the sparkle's colour (0.6) beside it. */
const SETTINGS_OPACITY = 0.6;

export class ExpandedView {
    constructor(pages, onWidth) {
        this._pages = pages;
        this._onWidth = onWidth;
        let saved = 0;
        try {
            saved = pages.findIndex(page => page.name === localStorage.getItem(PAGE_KEY));
        } catch {}
        this._index = Math.max(0, saved);
        this.node = el('div', 'isle-view isle-expanded');
        this.node.append(this._buildHeader());
        this._pager = el('div', 'isle-pager');
        for (const page of pages) {
            page.node.style.visibility = 'hidden';
            page.onWidth = () => page === this.page && this._onWidth?.();
            this._pager.append(page.node);
        }
        this.node.append(this._pager, el('div', 'isle-pager-bottom'));
        this._syncTabs();
        this._showNow();
        daemon.connect(key => {
            if (key === 'nowPlaying' || key === 'pomodoro' || key === 'settings')
                this._updateStatus();
            if (key === 'call')
                this._syncHidden();
            if (key === 'settings') {
                this._syncHidden();
                this._tickClock();
            }
        });
        this._syncHidden();
        watchClock(() => this._tickClock());
        this._updateStatus();
    }

    get page() {
        return this._pages[this._index];
    }

    get preferredWidth() {
        return this.page.preferredWidth ?? EXPANDED_WIDTH;
    }

    get height() {
        return HEADER_HEIGHT + PAGE_HEIGHT + BOTTOM_PADDING;
    }

    _buildHeader() {
        const header = el('div', 'isle-header');
        let wheel = 0, lastFlip = 0;
        header.addEventListener('wheel', event => {
            event.preventDefault();
            wheel += Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
            const now = performance.now();
            if (Math.abs(wheel) >= WHEEL_STEP && now - lastFlip >= WHEEL_REPEAT_MS) {
                this.showPage(this._index + Math.sign(wheel));
                wheel = 0;
                lastFlip = now;
            }
        }, {passive: false});

        const tabs = el('div', 'isle-tabs');
        this._tabs = this._pages.map((page, index) => {
            const {icon: iconName, label} = TABS[page.name];
            const glyph = icon(iconName, 16);
            const name = el('span', 'isle-tab-label', [label]);
            const clip = el('span', 'isle-tab-name', [name]);
            const tab = el('button', `isle-tab isle-tab-${page.name}`, [glyph, clip]);
            tab.setAttribute('aria-label', label);
            tab.addEventListener('click', () => this.showPage(index));
            // Something new on the page: a dot on its tab.
            tab.append(el('span', 'isle-tab-dot'));
            page.onNews = () => tab.classList.toggle('news', Boolean(page.hasNews));
            page.onNews();
            tabs.append(tab);
            return {tab, glyph, name, clip, iconName};
        });
        header.append(tabs);

        const status = el('div', 'isle-header-status');
        this._bars = new Bars({height: 14});
        this._ring = new Ring({size: 15, lineWidth: 2.5});
        this._date = el('span', 'isle-header-date');
        this._time = el('span', 'isle-header-time');
        this._meridiem = el('span', 'isle-header-meridiem');
        const clock = el('div', 'isle-header-clock', [
            this._ring.node,
            this._date,
            el('div', 'isle-header-time-box', [this._time, this._meridiem]),
        ]);
        // The gear by the clock: Isle's settings.
        const settings = iconButton('isle-settings', 'isle-button-flat isle-header-settings', () => invoke('open_settings'), {size: 15});
        settings.setAttribute('aria-label', 'Settings');
        settings.firstChild.style.opacity = SETTINGS_OPACITY;
        settings.addEventListener('pointerenter', () => (settings.firstChild.style.opacity = 1));
        settings.addEventListener('pointerleave', () => (settings.firstChild.style.opacity = SETTINGS_OPACITY));
        status.append(this._bars.node, clock, settings);
        header.append(status);
        return header;
    }

    _tickClock() {
        const {time, meridiem} = clockParts();
        this._date.textContent = longDate();
        this._time.textContent = time;
        this._meridiem.textContent = meridiem;
        this._meridiem.style.display = meridiem ? '' : 'none';
    }

    /** The music bars while a song is on, the focus ring while the timer is. */
    _updateStatus() {
        const np = daemon.state.nowPlaying;
        const music = showsClosed(np) && !this.pageNamed('music')?.hidden;
        this._bars.node.style.display = music ? '' : 'none';
        this._bars.playing = music && np.status === 'Playing' && this._active;
        this._bars.opacity = music && np.status === 'Playing' ? 1 : PAUSED_BARS_OPACITY;
        const timer = daemon.state.pomodoro;
        const timing = timer && timer.phase !== 'idle';
        this._ring.node.style.display = timing ? '' : 'none';
        clearInterval(this._ringTicker);
        if (timing) {
            const sync = () => {
                const t = daemon.state.pomodoro;
                const remaining = t.running ? Math.max(0, t.ends_at_ms - Date.now()) : t.remaining_ms;
                this._ring.set(t.duration_ms ? 1 - remaining / t.duration_ms : 0, timerColor(t.phase));
            };
            sync();
            if (timer.running)
                this._ringTicker = setInterval(sync, 5000);
        }
    }

    /** The open tab is lit, with its filled icon and its name. */
    _syncTabs() {
        this._tabs.forEach(({tab, glyph, name, clip, iconName}, index) => {
            const open = index === this._index;
            tab.classList.toggle('checked', open);
            setIcon(glyph, open ? `${iconName}-fill` : iconName);
            clip.style.width = open ? `${Math.ceil(name.getBoundingClientRect().width || name.scrollWidth) + 6}px` : '0';
        });
    }

    setActive(active) {
        this._active = active;
        this._updateStatus();
        for (const page of this._pages)
            page.setActive?.(active && page === this.page);
        if (active)
            requestAnimationFrame(() => this._syncTabs());
    }

    _showNow() {
        this._pages.forEach((page, index) => {
            page.node.style.visibility = index === this._index ? '' : 'hidden';
            page.node.style.transform = '';
            page.node.style.opacity = '1';
        });
    }

    /** The page called `name`, if there is one. */
    pageNamed(name) {
        return this._pages.find(page => page.name === name);
    }

    /** Pages left out in Settings (hidden-pages; the Usage page by usage-enabled, as Isle) lose their tabs. */
    _syncHidden() {
        const settings = daemon.state.settings ?? {};
        const hidden = settings['hidden-pages'] ?? [];
        for (const page of this._pages)
            // The Apps and Usage pages have their own switches (app-usage, usage-enabled), as in Isle.
            // The Call page only has a tab during a call.
            page.hidden = page.name === 'call' ? !daemon.state.call?.active || settings['call-activity'] === false
                : page.name === 'usage' ? settings['usage-enabled'] === false
                : page.name === 'apps' ? !settings['app-usage'] : hidden.includes(page.name);
        // At least one page stays.
        if (this._pages.every(page => page.hidden))
            this._pages.find(page => page.name !== 'call').hidden = false;
        this._tabs.forEach(({tab}, index) => (tab.style.display = this._pages[index].hidden ? 'none' : ''));
        if (this.page.hidden) {
            this._index = this._pages.findIndex(page => !page.hidden);
            this._showNow();
            this._onWidth?.();
        }
        requestAnimationFrame(() => this._syncTabs());
    }

    /** Turns to the page called `name` ('focus', 'music', …), if there is one. */
    showPageNamed(name) {
        const index = this._pages.findIndex(page => page.name === name);
        if (index >= 0 && !this._pages[index].hidden)
            this.showPage(index);
    }

    /** Turns to page `index`, sliding the old one out and the new one in from the side it is on; a page left out
     * is passed over, on in the same direction. */
    showPage(index) {
        const step = Math.sign(index - this._index);
        while (index >= 0 && index < this._pages.length && this._pages[index].hidden)
            index += step || 1;
        if (index < 0 || index >= this._pages.length)
            return;
        if (index === this._index)
            return;
        const direction = Math.sign(index - this._index);
        const from = this.page;
        this._index = index;
        const to = this.page;
        try {
            localStorage.setItem(PAGE_KEY, to.name);
        } catch {}
        const ease = `${SLIDE_DURATION}ms cubic-bezier(0.25, 1, 0.5, 1)`;
        from.node.style.transition = `transform ${ease}, opacity ${SLIDE_DURATION * 0.6}ms ease-out`;
        from.node.style.transform = `translateX(${-direction * SLIDE_DISTANCE}px)`;
        from.node.style.opacity = '0';
        from.setActive?.(false);
        clearTimeout(from._slide);
        from._slide = setTimeout(() => {
            if (from !== this.page)
                from.node.style.visibility = 'hidden';
        }, SLIDE_DURATION);
        clearTimeout(to._slide);
        to.node.style.transition = 'none';
        to.node.style.transform = `translateX(${direction * SLIDE_DISTANCE}px)`;
        to.node.style.opacity = '0';
        to.node.style.visibility = '';
        to.setActive?.(this._active);
        requestAnimationFrame(() => requestAnimationFrame(() => {
            to.node.style.transition = `transform ${ease}, opacity ${SLIDE_DURATION}ms ease-out`;
            to.node.style.transform = 'translateX(0)';
            to.node.style.opacity = '1';
        }));
        this._syncTabs();
        this._onWidth?.();
    }
}
