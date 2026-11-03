// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The Calendar page (gnome-extension/src/pages/calendar.js): the month on the left, the chosen day's events on the
// right, from Windows' calendars and Isle's own (removable); a field to add one in a line ("Dinner 8:00 PM"); and
// world clocks, as a strip under the events and in full in their place. The alerts before events come from the
// Rust side (calendar.rs `Alarms`).

import {daemon} from '../daemon.js';
import {el, icon, iconButton, timeOfDay, uses12Hour} from '../widgets.js';

const {invoke} = window.__TAURI__.core;

/** The month grid's height, shared by its rows; five weeks fill it, six take smaller days. */
const GRID_HEIGHT = 105;
const MAX_ROW_HEIGHT = 21;
/** Isle's own events (orange), and a calendar with no colour of its own (blue). */
const OWN_COLOR = '#ff9f0a';
const SYSTEM_COLOR = '#0a84ff';
/** An event just added shows at once, faint, until it is listed, or this long (ms) at most. */
const PENDING_MS = 30000;
const DAY_MS = 24 * 3600 * 1000;
/** Cities the strip under the events names; the rest are counted ("+2"). */
const STRIP_CITIES = 3;
/** The time the line typed reads as shows this long (ms) after the last key. */
const PREVIEW_DELAY_MS = 120;

const setting = key => daemon.state.settings?.[key];

/** "2026-10-21" for a local Date. */
function dayKey(date) {
    const pad = n => String(n).padStart(2, '0');
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function startOfDay(date) {
    return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

/** Midnight `days` later; by date, so a daylight saving change cannot shift it. */
function addDays(date, days) {
    return new Date(date.getFullYear(), date.getMonth(), date.getDate() + days);
}

/** `day` at "HH:MM". */
function atTime(day, time) {
    const [hours, minutes] = time.split(':').map(Number);
    return new Date(day.getFullYear(), day.getMonth(), day.getDate(), hours, minutes);
}

/** "09:00 – 10:00", or "9:00 – 10:00 AM" with a 12-hour clock: AM or PM once when both have it. */
function rangeText(start, end) {
    const [from, to] = [timeOfDay(start), timeOfDay(end)];
    const mark = text => text.replace(/^[\d:.\s]+/, '');
    return mark(from) && mark(from) === mark(to) ? `${from.slice(0, -mark(from).length).trim()} – ${to}` : `${from} – ${to}`;
}

/** Whether two of a day's events are one: the same title at the same time, or both all day. */
function sameEvent(a, b) {
    return a.title === b.title && a.allDay === b.allDay && (a.allDay || Math.abs(a.start - b.start) < 60000);
}

/** A city's sky at `hour`: day, evening (16:00 to 18:59) or night (19:00 to 5:59). */
function skyIcon(hour) {
    if (hour >= 19 || hour < 6)
        return 'isle-moon';
    return hour >= 16 ? 'isle-sunset-symbolic' : 'isle-sun-symbolic';
}

/** The wall clock in `timezone` at `date`: {year, month, day, hour, minute}. */
function wallClock(timezone, date) {
    const parts = Object.fromEntries(new Intl.DateTimeFormat('en-US', {timeZone: timezone, hourCycle: 'h23',
        year: 'numeric', month: 'numeric', day: 'numeric', hour: 'numeric', minute: 'numeric'})
        .formatToParts(date).map(({type, value}) => [type, Number(value)]));
    return parts;
}

/** The time in `timezone` as the island writes times ("14:05", "2:05 PM"). */
function timeIn(timezone, date) {
    const {year, month, day, hour, minute} = wallClock(timezone, date);
    return timeOfDay(new Date(year, month - 1, day, hour, minute));
}

/** "+2h", "−6h", "+5:30h": how far `timezone` is ahead of the local time; "" when the same. */
function offsetText(timezone, date) {
    const {year, month, day, hour, minute} = wallClock(timezone, date);
    const there = Date.UTC(year, month - 1, day, hour, minute);
    const here = Date.UTC(date.getFullYear(), date.getMonth(), date.getDate(), date.getHours(), date.getMinutes());
    const minutes = Math.round((there - here) / 60000);
    if (minutes === 0)
        return '';
    const abs = Math.abs(minutes);
    const sign = minutes > 0 ? '+' : '−';
    return abs % 60 ? `${sign}${Math.floor(abs / 60)}:${String(abs % 60).padStart(2, '0')}h` : `${sign}${abs / 60}h`;
}

/** "Tomorrow" or "Yesterday" when the city's date is not the local one, else "". */
function dayText(timezone, date) {
    const {year, month, day} = wallClock(timezone, date);
    const diff = Math.round((Date.UTC(year, month - 1, day) -
        Date.UTC(date.getFullYear(), date.getMonth(), date.getDate())) / DAY_MS);
    return diff > 0 ? 'Tomorrow' : diff < 0 ? 'Yesterday' : '';
}

export class CalendarPage {
    constructor() {
        this.name = 'calendar';
        this.node = el('div', 'isle-page isle-calendar');
        this._today = startOfDay(new Date());
        this._selected = this._today;
        this._month = new Date(this._today.getFullYear(), this._today.getMonth(), 1);
        this._pending = [];
        this._showingWorld = false;
        this._timers = {};
        this.node.append(this._buildMonth(), this._buildAgenda());
        daemon.connect(() => this._refresh(), 'events');
        daemon.connect(() => this._refresh(), 'systemEvents');
        daemon.connect(() => {
            this._shownMonth = this._shownAgenda = null;
            this._syncWorld();
            this._refresh();
        }, 'settings');
        this._scheduleMidnight();
        this._requestRange();
    }

    get preferredWidth() {
        return null;
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

    // Building

    /** "NOVEMBER 2026" in red (a click goes to today), the arrows, the weekdays and the grid. */
    _buildMonth() {
        this._monthName = el('span', 'isle-cal-month-name');
        this._year = el('span', 'isle-cal-year');
        const title = el('button', 'isle-cal-month-title', [this._monthName, this._year]);
        title.setAttribute('aria-label', 'Go to today');
        title.addEventListener('click', () => this._goToToday());
        const header = el('div', 'isle-cal-month-header', [title,
            iconButton('isle-chevron-left-symbolic', 'isle-button-flat isle-cal-arrow', () => this._moveMonth(-1), {size: 11, label: 'Previous month'}),
            iconButton('isle-chevron-right-symbolic', 'isle-button-flat isle-cal-arrow', () => this._moveMonth(1), {size: 11, label: 'Next month'}),
        ]);
        this._weekdays = el('div', 'isle-cal-weekdays');
        this._grid = el('div', 'isle-cal-grid');
        return el('div', 'isle-cal-month', [header, this._weekdays, this._grid]);
    }

    /** The day's name and Today, the field, the list (or "No events"), the world clocks. */
    _buildAgenda() {
        this._dayName = el('span', 'isle-cal-day-name');
        this._worldButton = iconButton('isle-globe', 'isle-button-flat isle-cal-world-toggle',
            () => this._setShowingWorld(!this._showingWorld), {size: 13, label: 'World clocks'});
        this._todayButton = el('button', 'isle-cal-today', ['Today']);
        this._todayButton.addEventListener('click', () => this._goToToday());
        const header = el('div', 'isle-cal-day-header', [this._dayName, this._worldButton, this._todayButton]);

        this._entry = el('input', 'isle-cal-add-text');
        this._entry.addEventListener('focus', () => window.isleKeyboard?.(true));
        this._entry.addEventListener('blur', () => window.isleKeyboard?.(false));
        this._entry.addEventListener('keydown', event => {
            if (event.key === 'Enter')
                this._add();
            else if (event.key === 'Escape')
                this._stopAdding();
        });
        this._entry.addEventListener('input', () => this._later('preview', PREVIEW_DELAY_MS, () => this._previewLine()));
        this._preview = el('span', 'isle-cal-preview');
        this._field = el('label', 'isle-entry isle-cal-add', [icon('isle-plus-symbolic', 13), this._entry, this._preview]);

        this._list = el('div', 'isle-cal-events');
        this._scroll = el('div', 'isle-cal-scroll', [this._list]);
        this._noEvents = el('div', 'isle-cal-no-events', [el('span', 'isle-cal-empty', ['No events'])]);
        this._worldList = el('div', 'isle-cal-events isle-cal-world');
        this._worldScroll = el('div', 'isle-cal-scroll', [this._worldList]);
        this._worldScroll.style.display = 'none';
        const listArea = el('div', 'isle-cal-list-area', [this._scroll, this._noEvents, this._worldScroll]);

        this._worldStrip = el('button', 'isle-cal-world-strip');
        this._worldStrip.addEventListener('click', () => this._setShowingWorld(true));
        this._syncWorld();
        return el('div', 'isle-cal-agenda', [header, this._field, listArea, this._worldStrip]);
    }

    // World clocks

    get _cities() {
        return setting('world-clocks') === false ? [] : setting('world-clock-cities') ?? [];
    }

    /** The globe and the strip while there are cities; with none left, back to the events. */
    _syncWorld() {
        const cities = this._cities;
        const key = JSON.stringify(cities);
        if (key !== this._worldKey) {
            this._worldKey = key;
            this._buildWorld(cities);
        }
        this._worldButton.style.display = cities.length ? '' : 'none';
        if (!cities.length && this._showingWorld)
            this._setShowingWorld(false);
        this._worldStrip.style.display = cities.length && !this._showingWorld ? '' : 'none';
    }

    _buildWorld(cities) {
        this._clocks = [];
        const chips = [];
        this._worldList.replaceChildren(...cities.map(({name, timezone}, i) => {
            const clock = {timezone, icon: icon('isle-sun-symbolic', 16), meta: el('span', 'isle-cal-world-meta'),
                time: el('span', 'isle-cal-world-time')};
            if (i < STRIP_CITIES) {
                clock.chipTime = el('span', 'isle-cal-world-chip-time');
                chips.push(el('span', 'isle-cal-world-chip', [el('span', 'isle-cal-world-chip-city', [name]), clock.chipTime]));
            }
            this._clocks.push(clock);
            clock.icon.classList.add('isle-cal-world-icon');
            return el('div', 'isle-cal-world-row', [clock.icon, el('span', 'isle-cal-world-city', [name]), clock.meta, clock.time]);
        }));
        if (cities.length > STRIP_CITIES)
            chips.push(el('span', 'isle-cal-world-chip-city', [`+${cities.length - STRIP_CITIES}`]));
        this._worldStrip.replaceChildren(...chips);
        this._tickWorld();
    }

    _tickWorld() {
        const now = new Date();
        for (const clock of this._clocks ?? []) {
            let shown;
            try {
                shown = timeIn(clock.timezone, now);
                clock.meta.textContent = [dayText(clock.timezone, now), offsetText(clock.timezone, now)].filter(Boolean).join(' · ');
                clock.icon.style.setProperty('--icon', `url("icons/${skyIcon(wallClock(clock.timezone, now).hour)}.svg")`);
            } catch {
                shown = '–';
            }
            clock.time.textContent = shown;
            if (clock.chipTime)
                clock.chipTime.textContent = shown;
        }
        if (this._active && this._clocks?.length)
            this._later('world', (60 - now.getSeconds()) * 1000 + 50, () => this._tickWorld());
    }

    _setShowingWorld(showing) {
        this._showingWorld = showing;
        this._worldButton.classList.toggle('checked', showing);
        if (showing)
            this._stopAdding();
        this._field.style.display = showing ? 'none' : '';
        this._worldScroll.style.display = showing ? '' : 'none';
        this._syncWorld();
        this._tickWorld();
        this._refresh(true);
    }

    // Events

    /** Asks for Windows' calendars over the month shown and the day picked (today and tomorrow are always read). */
    _requestRange() {
        const monthEnd = new Date(this._month.getFullYear(), this._month.getMonth() + 1, 1);
        // Weeks of the month before and after on show too, a day each side for time zones.
        const begin = addDays(new Date(Math.min(this._month, this._selected)), -7);
        const end = addDays(new Date(Math.max(monthEnd, addDays(this._selected, 1))), 7);
        const key = `${begin.getTime()}-${end.getTime()}`;
        if (key !== this._rangeKey) {
            this._rangeKey = key;
            daemon.call('Calendar1', 'Range', {begin_ms: begin.getTime(), end_ms: end.getTime()});
        }
    }

    /** Windows' events overlapping [begin, end). */
    _systemEvents(begin, end) {
        return (daemon.state.systemEvents?.events ?? []).filter(event =>
            event.start_ms < end.getTime() && Math.max(event.end_ms, event.start_ms + 1) > begin.getTime());
    }

    /** `{id, title, start, end, allDay, color, ownId, link}` for each event on `day`, all-day ones first, then by time. */
    _eventsOn(day) {
        const begin = startOfDay(day);
        const end = addDays(begin, 1);
        const events = this._systemEvents(begin, end).map(event => ({
            id: event.id,
            title: event.title,
            start: new Date(event.start_ms),
            end: new Date(event.end_ms),
            allDay: event.all_day || (event.start_ms <= begin.getTime() && event.end_ms >= end.getTime()),
            color: event.color || SYSTEM_COLOR,
            ownId: null,
            link: event.link,
        }));
        const key = dayKey(day);
        for (const event of daemon.state.events ?? []) {
            if (event.date !== key)
                continue;
            events.push({
                id: null,
                title: event.title,
                start: event.start ? atTime(begin, event.start) : begin,
                end: event.end ? atTime(begin, event.end) : null,
                allDay: !event.start,
                color: OWN_COLOR,
                ownId: event.id,
                link: '',
            });
        }
        // Just added: shown until it is listed, then that one is.
        const now = Date.now();
        this._pending = this._pending.filter(pending => pending.until > now &&
            (pending.date !== key || !events.some(event => sameEvent(event, pending))));
        for (const pending of this._pending) {
            if (pending.date === key)
                events.push({...pending, id: null, ownId: null, pending});
        }
        return events.sort((a, b) => (b.allDay - a.allDay) || (a.start - b.start));
    }

    /** The days of the month shown that have events, as day numbers. */
    _busyDays() {
        const begin = this._month;
        const end = new Date(begin.getFullYear(), begin.getMonth() + 1, 1);
        const busy = new Set();
        for (const event of this._systemEvents(begin, end)) {
            const from = new Date(Math.max(event.start_ms, begin.getTime()));
            const until = Math.max(event.end_ms, event.start_ms + 1);
            for (let day = startOfDay(from); day < end && day.getTime() < until; day = addDays(day, 1))
                busy.add(day.getDate());
        }
        const prefix = dayKey(begin).slice(0, 8);
        for (const event of daemon.state.events ?? []) {
            if (event.date.startsWith(prefix))
                busy.add(Number(event.date.slice(8)));
        }
        return busy;
    }

    // Showing

    setActive(active) {
        this._active = active;
        if (active) {
            if (!this._followToday())
                this._refresh();
            this._requestRange();
            this._tickWorld();
        } else {
            this._stopAdding();
            this._cancel('world');
        }
    }

    /** A new day since it was drawn: today is picked, unless another day was. */
    _followToday() {
        const today = startOfDay(new Date());
        if (today.getTime() === this._today.getTime() || this._selected.getTime() !== this._today.getTime())
            return false;
        this._select(today, {keepWorld: true});
        return true;
    }

    _refresh(force = false) {
        this._today = startOfDay(new Date());
        const busy = this._busyDays();
        const weekStart = setting('system-week-start') ?? 1;
        const month = JSON.stringify([this._today.getTime(), this._month.getTime(), [...busy], weekStart]);
        if (month !== this._shownMonth) {
            this._shownMonth = month;
            this._renderMonth(busy, weekStart);
        } else {
            for (const button of this._dayButtons)
                button.className = this._dayClass(button._day);
        }
        this._renderDayHeader();
        const now = new Date();
        const events = this._eventsOn(this._selected);
        const agenda = JSON.stringify([this._selected.getTime(), uses12Hour(),
            events.map(e => [e.title, +e.start, +e.end, e.allDay, e.ownId, !e.allDay && (e.end ?? e.start) < now, e.color, Boolean(e.pending)])]);
        if (agenda !== this._shownAgenda || force) {
            this._shownAgenda = agenda;
            this._renderAgenda(events);
        }
        const showingEvents = !this._showingWorld;
        this._noEvents.style.display = showingEvents && !events.length ? '' : 'none';
        this._scroll.style.display = showingEvents && events.length ? '' : 'none';
    }

    /** The weekday letters and the grid, with a dot under each day in `busy`. */
    _renderMonth(busy, weekStart) {
        const month = this._month;
        this._monthName.textContent = month.toLocaleDateString('en-GB', {month: 'long'}).toUpperCase();
        this._year.textContent = String(month.getFullYear());
        this._weekend = [0, 6];
        // 4 Jan 1970 was a Sunday.
        const days = [...Array(7).keys()].map(i => new Date(1970, 0, 4 + (weekStart + i) % 7));
        this._weekdays.replaceChildren(...days.map(day => el('span',
            `isle-cal-weekday${this._weekend.includes(day.getDay()) ? ' weekend' : ''}`,
            [day.toLocaleDateString('en-GB', {weekday: 'short'}).slice(0, 1).toUpperCase()])));

        const daysInMonth = new Date(month.getFullYear(), month.getMonth() + 1, 0).getDate();
        const lead = (month.getDay() - weekStart + 7) % 7;
        const rows = Math.ceil((lead + daysInMonth) / 7);
        const rowHeight = Math.min(MAX_ROW_HEIGHT, Math.floor(GRID_HEIGHT / rows));
        this._grid.classList.toggle('isle-cal-six-weeks', rows > 5);
        this._dayButtons = [];
        const weeks = [];
        for (let row = 0; row < rows; row++) {
            const cells = [];
            for (let column = 0; column < 7; column++) {
                const number = row * 7 + column - lead + 1;
                cells.push(number >= 1 && number <= daysInMonth
                    ? this._dayCell(new Date(month.getFullYear(), month.getMonth(), number), busy.has(number))
                    : el('div', 'isle-cal-cell'));
            }
            const week = el('div', 'isle-cal-week', cells);
            week.style.height = `${rowHeight}px`;
            weeks.push(week);
        }
        this._grid.replaceChildren(...weeks);
    }

    _dayClass(day) {
        const classes = ['isle-cal-day'];
        if (day.getTime() === this._today.getTime())
            classes.push('today');
        else if (day.getTime() === this._selected.getTime())
            classes.push('selected');
        else if (this._weekend.includes(day.getDay()))
            classes.push('weekend');
        return classes.join(' ');
    }

    _dayCell(day, busy) {
        const button = el('button', this._dayClass(day), [String(day.getDate())]);
        button._day = day;
        button.setAttribute('aria-label', day.toLocaleDateString('en-GB', {weekday: 'long', day: 'numeric', month: 'long'}));
        button.addEventListener('click', () => this._select(day));
        this._dayButtons.push(button);
        const cell = el('div', 'isle-cal-cell', [button]);
        if (busy)
            cell.append(el('div', `isle-cal-dot${day.getTime() === this._today.getTime() ? ' on-today' : ''}`));
        return cell;
    }

    /** "Today, 21 October", and Today when another day or month is shown; the field's hint. */
    _renderDayHeader() {
        const day = this._selected;
        const offset = Math.round((day - this._today) / DAY_MS);
        const name = {'-1': 'Yesterday', 0: 'Today', 1: 'Tomorrow'}[offset] ?? day.toLocaleDateString('en-GB', {weekday: 'long'});
        this._dayName.textContent = this._showingWorld ? 'World Clocks'
            : `${name}, ${day.toLocaleDateString('en-GB', {day: 'numeric', month: 'long'})}`;
        this._todayButton.style.display = !this._showingWorld && (offset !== 0 ||
            this._month.getMonth() !== this._today.getMonth() || this._month.getFullYear() !== this._today.getFullYear()) ? '' : 'none';
        const eight = uses12Hour() ? '8:00 PM' : '20:00';
        this._entry.placeholder = `Add event · e.g. Dinner ${eight}`;
    }

    _renderAgenda(events) {
        const day = this._selected;
        if (day.getTime() !== this._listedDay?.getTime())
            this._scroll.scrollTop = 0;
        this._listedDay = day;
        const now = new Date();
        const removable = events.some(event => event.ownId !== null);
        const rows = events.map(event => this._eventRow(event, now, removable));
        this._list.replaceChildren(...rows);
        // The one just added, scrolled to: as added, or as it is listed already.
        if (!this._reveal)
            return;
        const reveal = events.findIndex(event => event.pending === this._reveal || (!event.pending && sameEvent(event, this._reveal)));
        if (reveal >= 0) {
            this._reveal = null;
            rows[reveal].scrollIntoView({block: 'nearest', behavior: 'smooth'});
        }
    }

    /** A row: its calendar's colour as a dot, the title, the time, and × for Isle's own. */
    _eventRow(event, now, removable) {
        const over = !event.allDay && (event.end ?? event.start) < now;
        const dot = el('span', 'isle-cal-event-dot');
        dot.style.backgroundColor = event.color;
        let when = 'All day';
        if (!event.allDay) {
            const startsToday = event.start >= startOfDay(this._selected);
            when = startsToday ? timeOfDay(event.start) : `until ${timeOfDay(event.end)}`;
            if (startsToday && event.end && event.end > event.start && event.end < addDays(startOfDay(this._selected), 1))
                when = rangeText(event.start, event.end);
        }
        const row = el('div', `isle-cal-event${event.pending ? ' pending' : over ? ' over' : ''}`,
            [dot, el('span', 'isle-cal-event-title', [event.title]), el('span', 'isle-cal-event-time', [when])]);
        if (event.ownId !== null) {
            let removing = false;
            row.append(iconButton('isle-close-symbolic', 'isle-button-flat isle-reminder-remove isle-cal-remove', () => {
                if (!removing) {
                    removing = true;
                    daemon.call('Calendar1', 'Remove', {id: event.ownId});
                }
            }, {size: 10, label: 'Remove event'}));
        } else if (removable) {
            row.append(el('span', 'isle-cal-remove-room'));
        }
        return row;
    }

    // Doing

    _select(day, {keepWorld = false} = {}) {
        if (this._showingWorld && !keepWorld)
            this._setShowingWorld(false);
        this._selected = day;
        if (day.getMonth() !== this._month.getMonth() || day.getFullYear() !== this._month.getFullYear())
            this._month = new Date(day.getFullYear(), day.getMonth(), 1);
        this._requestRange();
        this._refresh();
    }

    _moveMonth(step) {
        this._month = new Date(this._month.getFullYear(), this._month.getMonth() + step, 1);
        this._requestRange();
        this._refresh();
    }

    _goToToday() {
        this._select(startOfDay(new Date()));
    }

    /** How the line reads (`{title, start, end}`), or null. */
    async _parse(text) {
        try {
            return await invoke('calendar_parse', {text});
        } catch {
            return null;
        }
    }

    /** Adds the typed event to Isle's own calendar on the picked day, showing it at once. */
    async _add() {
        const text = this._entry.value.trim();
        if (!text)
            return;
        const day = this._selected;
        this._entry.value = '';
        this._preview.textContent = '';
        const event = await this._parse(text);
        if (!event?.title) {
            // A time alone: back in the field to fix.
            if (!this._entry.value)
                this._entry.value = text;
            return;
        }
        const begin = startOfDay(day);
        const start = event.start ? atTime(begin, event.start) : begin;
        const pending = {date: dayKey(day), title: event.title, start, end: event.end ? atTime(begin, event.end) : null,
            allDay: !event.start, color: OWN_COLOR, until: Date.now() + PENDING_MS};
        this._pending.push(pending);
        this._reveal = pending;
        this._refresh();
        daemon.call('Calendar1', 'Add', {date: dayKey(day), text});
    }

    /** Shows in the field when the line typed takes place ("8:00 PM", "All day"), as it will be read. */
    async _previewLine() {
        const typed = this._entry.value.trim();
        const event = typed ? await this._parse(typed) : null;
        if (typed !== this._entry.value.trim())
            return;
        const begin = startOfDay(this._selected);
        const at = time => atTime(begin, time);
        this._preview.textContent = !event ? '' : !event.start ? 'All day'
            : event.end ? rangeText(at(event.start), at(event.end)) : timeOfDay(at(event.start));
    }

    _stopAdding() {
        this._entry.value = '';
        this._preview.textContent = '';
        this._cancel('preview');
        if (document.activeElement === this._entry)
            this._entry.blur();
    }

    /** Redraws at midnight, so "today" moves on. */
    _scheduleMidnight() {
        const now = new Date();
        this._later('midnight', addDays(startOfDay(now), 1) - now + 1000, () => {
            if (!this._followToday())
                this._refresh();
            this._requestRange();
            this._scheduleMidnight();
        });
    }
}
