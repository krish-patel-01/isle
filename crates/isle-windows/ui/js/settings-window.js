// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// Isle's Settings window (gnome-extension/prefs.js): Island, Timer, AI Usage and About, in that order, with the
// rows that mean something on Windows. Each row shows its setting and changes it at once; changes from elsewhere
// (Reset, the island) show here as they happen.

import {daemon} from './daemon.js';

const {invoke} = window.__TAURI__.core;

/** Edits to the timer are sent once typing pauses for this long. */
const SAVE_DELAY_MS = 400;
const PAGE_KEY = 'isle.settings.page';

/** Every row's way of showing its value anew. */
const syncs = [];
const setting = key => daemon.state.settings?.[key];

function el(tag, className = '', children = []) {
    const node = document.createElement(tag);
    if (className)
        node.className = className;
    for (const child of [].concat(children))
        node.append(child);
    return node;
}

function icon(name) {
    const node = el('span', 'icon');
    node.style.setProperty('--icon', `url("icons/${name}.svg")`);
    return node;
}

async function set(key, value) {
    try {
        await invoke('settings_set', {key, value});
    } catch (err) {
        toast(String(err));
        syncAll();
    }
}

function syncAll() {
    for (const sync of syncs)
        sync();
}

function toast(text) {
    const node = el('div', 'toast', [text]);
    document.body.append(node);
    setTimeout(() => node.remove(), 3000);
}

// Building blocks

function page(name, title, iconName, groups) {
    const node = el('section', 'page', [el('div', 'clamp', groups.filter(Boolean))]);
    node.dataset.name = name;
    return {name, title, iconName, node};
}

function group(title, description, rows) {
    const head = el('div', 'group-head', [el('div', 'group-title', [title])]);
    if (description)
        head.append(el('div', 'group-description', [description]));
    return el('div', 'group', [head, el('div', 'list', rows.filter(Boolean))]);
}

/** A row: its title over a dimmer subtitle, things after it, an icon before it if given. */
function row(title, subtitle, suffix = [], {prefix = null} = {}) {
    const texts = el('div', 'row-texts', [el('div', 'row-title', [title]), el('div', 'row-subtitle', [subtitle ?? ''])]);
    const node = el('div', 'row', [...prefix ? [el('div', 'row-prefix', [icon(prefix)])] : [], texts,
        el('div', 'row-suffix', [].concat(suffix))]);
    node.setSubtitle = text => (texts.lastChild.textContent = text ?? '');
    node.setSensitive = sensitive => node.classList.toggle('insensitive', !sensitive);
    return node;
}

/** A switch bound to a boolean key, or to `get` and `put`. */
function switchRow(key, title, subtitle, {prefix, get = () => setting(key), put = value => set(key, value)} = {}) {
    const toggle = el('button', 'switch');
    toggle.setAttribute('role', 'switch');
    const node = row(title, subtitle, toggle, {prefix});
    node.classList.add('activatable');
    node.addEventListener('click', () => put(toggle.getAttribute('aria-checked') !== 'true'));
    syncs.push(() => toggle.setAttribute('aria-checked', String(Boolean(get()))));
    return node;
}

/** A drop-down for a string key with `choices`: [[value, label], …]. */
function choiceRow(key, title, subtitle, choices) {
    const value = el('span', '', ['']);
    const button = el('button', 'combo', [value, icon('isle-chevron-down-symbolic')]);
    const node = row(title, subtitle, button);
    node.classList.add('activatable');
    node.addEventListener('click', event => {
        event.stopPropagation();
        openPopover(button, choices.map(([choice, label]) => ({label, checked: choice === setting(key),
            pick: () => set(key, choice)})));
    });
    syncs.push(() => (value.textContent = choices.find(([choice]) => choice === setting(key))?.[1] ?? choices[0][1]));
    return node;
}

let popover = null;

function closePopover() {
    popover?.remove();
    popover = null;
}

function openPopover(anchor, items) {
    closePopover();
    popover = el('div', 'popover', items.map(({label, checked, pick}) => {
        const item = el('button', '', [el('span', '', [label]), el('span', 'check', [icon('isle-check-symbolic')])]);
        item.setAttribute('aria-checked', String(checked));
        item.addEventListener('click', event => {
            event.stopPropagation();
            closePopover();
            pick();
        });
        return item;
    }));
    document.body.append(popover);
    const box = anchor.getBoundingClientRect();
    const width = popover.offsetWidth, height = popover.offsetHeight;
    popover.style.left = `${Math.max(8, Math.min(window.innerWidth - width - 8, box.right - width))}px`;
    const below = box.bottom + 4;
    popover.style.top = `${below + height > window.innerHeight - 8 ? Math.max(8, box.top - height - 4) : below}px`;
}

document.addEventListener('click', closePopover);
window.addEventListener('blur', closePopover);
document.addEventListener('keydown', event => event.key === 'Escape' && closePopover());

/** Whole numbers from `min` to `max` in steps of `step`, with − and +. */
function spinRow(title, subtitle, {min, max, step = 1, get, put}) {
    const input = el('input');
    input.type = 'number';
    Object.assign(input, {min, max, step});
    const minus = el('button', 'round', [icon('isle-minus-symbolic')]);
    const plus = el('button', 'round', [icon('isle-plus-symbolic')]);
    const node = row(title, subtitle, el('div', 'spin', [input, minus, plus]));
    const commit = value => {
        value = Math.max(min, Math.min(max, Math.round(value / step) * step));
        if (!Number.isFinite(value))
            return;
        input.value = value;
        if (value !== get())
            put(value);
        sync();
    };
    const sync = () => {
        const value = get();
        if (document.activeElement !== input)
            input.value = value;
        minus.disabled = value <= min;
        plus.disabled = value >= max;
    };
    minus.addEventListener('click', () => commit(Number(input.value) - step));
    plus.addEventListener('click', () => commit(Number(input.value) + step));
    input.addEventListener('change', () => commit(Number(input.value)));
    input.addEventListener('keydown', event => event.key === 'Enter' && input.blur());
    syncs.push(sync);
    return node;
}

/** A spin row bound to an int key. */
function intRow(key, title, subtitle, min, max, step = 1) {
    return spinRow(title, subtitle, {min, max, step, get: () => setting(key), put: value => set(key, value)});
}

/** A row that does something when clicked; an icon after it says what. */
function actionRow(title, subtitle, iconName, onActivate) {
    const node = row(title, subtitle, iconName ? icon(iconName) : []);
    node.classList.add('activatable');
    node.addEventListener('click', onActivate);
    return node;
}

/** Asks before something that can't be undone (Adw.AlertDialog). */
function confirm({heading, body, action}, onConfirm) {
    const cancel = el('button', '', ['Cancel']);
    const go = el('button', 'destructive', [action]);
    const scrim = el('div', 'scrim', [el('div', 'dialog', [el('h2', '', [heading]), el('p', '', [body]),
        el('div', 'buttons', [cancel, go])])]);
    const close = () => scrim.remove();
    cancel.addEventListener('click', close);
    go.addEventListener('click', () => {
        close();
        onConfirm();
    });
    scrim.addEventListener('click', event => event.target === scrim && close());
    document.body.append(scrim);
    cancel.focus();
}

/** A row shown only while `when()` holds, and sensitive only while `enabled()` does. */
function depends(node, {when = () => true, enabled = () => true} = {}) {
    syncs.push(() => {
        node.hidden = !when();
        node.setSensitive(enabled());
    });
    return node;
}

// Pages

/** The open island's pages that can be left out, in tab order, with what goes with each. */
const PAGES = [
    ['music', 'Music', 'isle-music', 'The player and lyrics, and the song in the closed island'],
    ['notifications', 'Notifications', 'isle-bell', 'The list to look back at, and its unread dot; alerts show either way'],
    ['focus', 'Timer', 'isle-timer', 'Pomodoro focus sessions and breaks'],
    ['reminders', 'Reminders', 'isle-tasks', 'Your to-do list'],
    ['calendar', 'Calendar', 'isle-calendar', 'The month, its events and world clocks'],
];

/** A switch per page; the last one on can't be turned off, so the island always has a page. */
function pagesGroup() {
    const hidden = () => setting('hidden-pages') ?? [];
    const shown = () => PAGES.filter(([name]) => !hidden().includes(name)).length + (setting('usage-enabled') ? 1 : 0) +
        (setting('app-usage') ? 1 : 0);
    const rows = PAGES.map(([name, title, iconName, subtitle]) => {
        const node = switchRow(null, title, subtitle, {
            prefix: iconName,
            get: () => !hidden().includes(name),
            put: on => set('hidden-pages', on ? hidden().filter(other => other !== name) : [...hidden(), name]),
        });
        syncs.push(() => {
            const last = shown() === 1 && !hidden().includes(name);
            node.setSensitive(!last);
            node.setSubtitle(last ? 'The island keeps at least one page' : subtitle);
        });
        return node;
    });
    // The Apps page's own switch: off as Isle comes, so nothing is timed until asked.
    const apps = switchRow('app-usage', 'Apps', 'Records the time spent in each app, kept on this computer for 30 days',
        {prefix: 'isle-apps'});
    // The Usage page's own switch, here too, so every tab is in one place.
    const usage = switchRow('usage-enabled', 'AI Usage', 'Your AI plans’ limits, and warnings as one nears its end',
        {prefix: 'isle-gauge'});
    syncs.push(() => {
        const found = setting('found-claude') || setting('found-codex');
        const last = shown() === 1 && setting('usage-enabled');
        usage.setSensitive(Boolean(found) && !last);
        usage.setSubtitle(!found ? 'Sign in to Claude Code or Codex to see your plan'
            : last ? 'The island keeps at least one page' : 'Your AI plans’ limits, and warnings as one nears its end');
    });
    syncs.push(() => {
        const last = shown() === 1 && setting('app-usage');
        apps.setSensitive(!last);
    });
    return group('Pages', 'The tabs of the open island. Turn off the ones you don’t use.', [...rows, apps, usage]);
}

/** What the Apps page keeps, and a way to forget it; its switch is with the other pages. */
function appUsageGroup() {
    const clear = el('button', 'pill-button', ['Delete…']);
    clear.addEventListener('click', event => {
        event.stopPropagation();
        confirm({
            heading: 'Delete app usage data?',
            body: 'Every app’s time recorded so far is removed from this computer. This cannot be undone.',
            action: 'Delete',
        }, () => daemon.call('AppUsage1', 'Clear'));
    });
    // Said in full: this is a record of what the user does, so what is kept, where, and for how long.
    return group('App Usage', 'While the Apps page is on, Isle records which app is in front and for how long, day ' +
        'by day: the app’s name and program, never what you type or see. It is kept in a file on this computer ' +
        '(%LOCALAPPDATA%\\Isle\\app-usage.json) for 30 days and never sent anywhere. Five minutes without input ' +
        'pause it, unless a video keeps the screen awake. Turning the page off stops recording; what was recorded stays ' +
        'until you delete it.', [row('Delete app usage data', 'Every day recorded so far, from this computer', clear)]);
}

/** The cities whose time the Calendar page shows, each with its time and a remove button, and a search to add one. */
function worldClocksGroup() {
    const cities = () => setting('world-clock-cities') ?? [];
    const list = el('div', 'world-cities');
    const add = actionRow('Add a City…', null, null, () => searchCity(city => {
        if (!cities().some(other => other.timezone === city.timezone))
            set('world-clock-cities', [...cities(), city]);
    }));
    add.querySelector('.row-texts').before(el('div', 'row-prefix', [icon('isle-plus-symbolic')]));
    const box = group('World Clocks', 'The time in other cities on the Calendar page.', [
        switchRow('world-clocks', 'Show world clocks', 'Under the day’s events; click them for more'),
        add,
    ]);
    syncs.push(() => {
        const rows = cities().map(city => {
            const remove = el('button', 'round flat', [icon('isle-minus-symbolic')]);
            remove.title = 'Remove';
            remove.addEventListener('click', () => set('world-clock-cities', cities().filter(other => other.timezone !== city.timezone)));
            let now = '';
            try {
                now = new Intl.DateTimeFormat(undefined, {timeZone: city.timezone, hour: 'numeric', minute: '2-digit'}).format(new Date());
            } catch {}
            const node = row(city.name, now, remove);
            node.classList.toggle('insensitive', setting('world-clocks') === false);
            return node;
        });
        // In the list, before "Add a City…".
        box.querySelectorAll('.list > .city').forEach(node => node.remove());
        for (const node of rows) {
            node.classList.add('city');
            add.before(node);
        }
    });
    return box;
}

/** Every time zone Windows knows as a city: "Asia/Kolkata" is Kolkata, in Asia. */
function allCities() {
    let zones = [];
    try {
        zones = Intl.supportedValuesOf('timeZone');
    } catch {}
    return zones.filter(zone => zone.includes('/') && !zone.startsWith('Etc/')).map(zone => {
        const parts = zone.split('/');
        const name = parts.at(-1).replaceAll('_', ' ');
        const place = parts.slice(0, -1).join(', ').replaceAll('_', ' ');
        return {name, place, timezone: zone, search: `${name} ${place}`.toLowerCase()};
    }).sort((a, b) => a.name.localeCompare(b.name));
}

/** A dialog to find a city by name; `pick` gets {name, timezone}. */
function searchCity(pick) {
    const field = el('input', 'search');
    field.placeholder = 'Search for a city';
    const results = el('div', 'results');
    const close = () => scrim.remove();
    const scrim = el('div', 'scrim', [el('div', 'dialog city-dialog', [el('h2', '', ['Add a City']), field, results])]);
    const cities = allCities();
    const show = () => {
        const query = field.value.trim().toLowerCase();
        const found = query ? cities.filter(city => city.search.includes(query)).slice(0, 40) : [];
        results.replaceChildren(...found.map(city => {
            const item = actionRow(city.name, city.place, null, () => {
                close();
                pick({name: city.name, timezone: city.timezone});
            });
            return item;
        }));
        if (query && !found.length)
            results.append(el('p', 'none', ['No city found']));
    };
    field.addEventListener('input', show);
    field.addEventListener('keydown', event => event.key === 'Escape' && close());
    scrim.addEventListener('click', event => event.target === scrim && close());
    document.body.append(scrim);
    field.focus();
}

/** How dark the glass is, Clear to Dark, on a slider; shown only while the island is glass. */
function glassRow() {
    const slider = el('input', 'slider');
    Object.assign(slider, {type: 'range', min: 0, max: 100, step: 5});
    const node = row('Glass', '', el('div', 'slider-ends', [el('span', 'end', ['Clear']), slider, el('span', 'end', ['Dark'])]));
    const describe = value => node.setSubtitle(value <= 0 ? 'Clear glass: what is behind shows through'
        : value >= 100 ? 'Dark frosted glass' : `${value}% darker`);
    slider.addEventListener('input', () => describe(Number(slider.value)));
    slider.addEventListener('change', () => set('glass-tint', Number(slider.value)));
    syncs.push(() => {
        node.hidden = setting('island-theme') !== 'glass';
        if (document.activeElement !== slider)
            slider.value = setting('glass-tint') ?? 0;
        describe(Number(slider.value));
    });
    return node;
}

function islandPage() {
    const delay = depends(intRow('hover-delay', 'Rest before opening', 'In milliseconds', 60, 1000, 20),
        {enabled: () => setting('open-on') === 'hover'});
    const hideDelay = depends(intRow('auto-hide-delay', 'Hide after', 'Seconds without the pointer on the island', 1, 60),
        {enabled: () => ['always', 'windows'].includes(setting('auto-hide'))});
    return page('island', 'Island', 'isle-display-symbolic', [
        group('Startup', null, [
            switchRow('start-with-windows', 'Start with Windows', 'Isle opens on its own when you sign in'),
        ]),
        group('Appearance', null, [
            choiceRow('island-theme', 'Island', 'Black, or smoked glass that lets what is behind it show through', [
                ['black', 'Black'],
                ['glass', 'Glass'],
            ]),
            glassRow(),
            intRow('notch-depth', 'Island height', 'How far the closed island hangs from the top of the screen, in pixels', 24, 40),
            choiceRow('clock-format', 'Clock', 'The time in the island and its pages', [
                ['system', 'As Windows'],
                ['24h', '24-hour (20:40)'],
                ['12h', '12-hour (8:40 PM)'],
            ]),
            choiceRow('island-monitor', 'Display', 'With more than one display, where the island is', [
                ['primary', 'Main display'],
                ['active', 'Display in use'],
            ]),
            choiceRow('open-on', 'Open', 'How the closed island opens', [
                ['hover', 'On hover'],
                ['click', 'On click'],
            ]),
            delay,
            choiceRow('auto-hide', 'Auto-hide', 'Hide the closed island; alerts still show, and pointing at its place brings it back', [
                ['off', 'Off'],
                ['always', 'Always'],
                ['windows', 'Beside windows'],
            ]),
            hideDelay,
        ]),
        pagesGroup(),
        appUsageGroup(),
        worldClocksGroup(),
        group('Alerts', null, [
            switchRow('notification-alerts', 'Notifications', 'Apps’ notifications in the island too, as Windows shows them'),
            switchRow('alerts-over-fullscreen', 'Over full-screen apps',
                'Alerts show over a video or game, then hide again; off, they wait until it leaves full screen'),
            switchRow('event-alerts', 'Calendar events', 'Before each event with a time, with Join for a Meet, Zoom or Teams link'),
            depends(intRow('event-alert-minutes', 'Minutes before an event', null, 1, 60),
                {enabled: () => setting('event-alerts')}),
            switchRow('track-peek', 'New song', 'Briefly show the song when the track changes'),
            switchRow('lyrics-online', 'Lyrics', 'Look up the playing song’s lyrics on LRCLIB'),
            switchRow('chimes', 'Sounds', 'Chime when a focus session or break ends'),
        ]),
    ]);
}

/** The timer's lengths live with the timer; changes are saved as typing pauses. */
function timerPage() {
    let config = null, saveId = 0;
    const save = () => {
        clearTimeout(saveId);
        saveId = setTimeout(() => {
            saveId = 0;
            daemon.call('pomodoro', 'SetConfig', config);
        }, SAVE_DELAY_MS);
    };
    // Don't lose the last change when the window closes before it is saved.
    window.addEventListener('beforeunload', () => saveId && daemon.call('pomodoro', 'SetConfig', config));
    daemon.connect(() => {
        if (!saveId) {
            config = {...daemon.state.pomodoroConfig};
            syncAll();
        }
    }, 'pomodoroConfig');
    const number = (key, title, min, max) => spinRow(title, null, {min, max,
        get: () => config?.[key] ?? min,
        put: value => {
            config[key] = value;
            save();
        }});
    const toggle = (key, title, subtitle) => switchRow(null, title, subtitle, {
        get: () => config?.[key],
        put: value => {
            config[key] = value;
            save();
            syncAll();
        }});
    return page('timer', 'Timer', 'isle-timer', [
        group('Pomodoro', 'Focus sessions with short breaks, and a long break after a few sessions.', [
            number('focus_minutes', 'Focus (minutes)', 1, 180),
            number('short_break_minutes', 'Short break (minutes)', 1, 60),
            number('long_break_minutes', 'Long break (minutes)', 1, 120),
            number('sessions_before_long_break', 'Sessions before a long break', 1, 12),
            toggle('auto_start_breaks', 'Start breaks automatically', null),
            toggle('auto_start_focus', 'Start focus automatically', 'After a break ends'),
        ]),
    ]);
}

/** AI plan usage: only the providers signed in on this computer are listed; then the agents found. */
function usagePage() {
    const found = () => Boolean(setting('found-claude') || setting('found-codex'));
    const usageOn = () => Boolean(setting('usage-enabled'));
    const description = el('span', '', ['']);
    const usage = group('AI Usage', null, [
        depends(switchRow('usage-enabled', 'Show AI usage', 'The Usage page and its warnings'), {enabled: found}),
        depends(switchRow('usage-claude', 'Claude', 'From Claude Code’s sign-in'),
            {when: () => setting('found-claude'), enabled: usageOn}),
        depends(switchRow('usage-codex', 'ChatGPT', 'From Codex’s sign-in'),
            {when: () => setting('found-codex'), enabled: usageOn}),
        depends(intRow('usage-warn', 'Warn at', 'Share of a limit, in %', 50, 99), {when: found, enabled: usageOn}),
        depends(intRow('usage-critical', 'Warn again at', 'Share of a limit, in %', 50, 100), {when: found, enabled: usageOn}),
    ]);
    usage.querySelector('.group-head').append(el('div', 'group-description', [description]));
    syncs.push(() => (description.textContent = found()
        ? 'How much of your plans’ limits is used, on a Usage page in the open island, with a warning as one nears ' +
          'its end. Found from the apps you are signed in to; nothing else to set up.'
        : 'Sign in to Claude Code or Codex and your plan shows up here.'));
    // Only the AIs found on this computer; none, and the group is not there at all.
    const agents = group('AI Agents', 'Answer your coding agents from the island when they ask, and know when they ' +
        'are done. Off, Isle stops getting that agent’s alerts.', [
        switchRow('agent-alerts-claude', 'Claude Code', 'Allow, deny or always allow, and when it is done'),
    ]);
    syncs.push(() => (agents.hidden = !setting('found-claude-code')));
    return page('usage', 'AI Usage', 'isle-gauge', [usage, agents]);
}

const WEBSITE = 'https://github.com/zickrian/isle';

function aboutPage() {
    const version = el('span', '', ['Isle']);
    invoke('plugin:app|version').then(v => (version.textContent = `Isle ${v}`)).catch(() => {});
    const about = group('', 'A notch-style island, for Windows. © 2026 zickrian, free software under the GPL.', [
        actionRow('Website', 'Features, installing and updating', 'isle-open-symbolic', () => invoke('open_link', {url: WEBSITE})),
        actionRow('Report a problem', 'On GitHub', 'isle-open-symbolic',
            () => invoke('open_link', {url: `${WEBSITE}/issues/new/choose`})),
    ]);
    about.querySelector('.group-title').replaceChildren(version);
    const reset = actionRow('Reset all settings', 'Back to how Isle came; your data stays', null, () => confirm({
        heading: 'Reset all of Isle’s settings?',
        body: 'Every option goes back to its default. Your tasks and timer history stay.',
        action: 'Reset',
    }, () => {
        invoke('settings_reset');
        // The timer's lengths, kept with the timer: an empty config is its defaults.
        daemon.call('pomodoro', 'SetConfig', {});
    }));
    return page('about', 'About', 'isle-info-symbolic', [about, group('Settings', null, [reset])]);
}

// The window

const pages = [islandPage(), timerPage(), usagePage(), aboutPage()];
const switcher = document.getElementById('switcher');
const main = document.getElementById('pages');
const buttons = pages.map(({name, title, iconName, node}) => {
    main.append(node);
    const button = el('button', '', [icon(iconName), el('span', '', [title])]);
    button.setAttribute('role', 'tab');
    button.addEventListener('click', () => open(name));
    switcher.append(button);
    return button;
});

function open(name) {
    pages.forEach(({name: each, node}, index) => {
        node.classList.toggle('open', each === name);
        buttons[index].setAttribute('aria-selected', String(each === name));
    });
    main.scrollTop = 0;
    try {
        localStorage.setItem(PAGE_KEY, name);
    } catch {}
}

let first = 'island';
try {
    first = localStorage.getItem(PAGE_KEY) ?? first;
} catch {}
open(pages.some(({name}) => name === first) ? first : 'island');

daemon.connect(syncAll, 'settings');
daemon.start().then(syncAll);
