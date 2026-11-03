// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// Alerts in the island (gnome-extension/src/peek.js and alerts.js): a round tinted badge with an icon, a title
// over a dimmer line, and a button when there is something to do. One that asks (an AI's permission) is wider:
// the command asked for under its words, and its answers below.

import {Art, Ring, el, icon, timeOfDay} from './widgets.js';

/** Widest a plain alert gets, its padding all round, and its height: a 32 px badge in 12 px of room. */
const MAX_WIDTH = 380;
const PADDING = 12;
export const PEEK_HEIGHT = 32 + 2 * PADDING;
/** How long a plain alert stays, and one with something to do (alerts.js STATUS_ and REMINDER_DURATION). */
const STATUS_DURATION = 2600;
export const REMINDER_DURATION = 8000;
/** The ring starts sweeping once the island has made room, and takes this long. */
const RING_DELAY = 350;
const RING_SWEEP = 700;
/** An answer shows this long ("Allowed") before the alert goes. */
const ANSWERED_MS = 850;

const TONES = {
    neutral: [255, 255, 255],
    good: [48, 209, 88],
    focus: [48, 209, 88],
    bad: [255, 69, 58],
    warn: [255, 159, 10],
    claude: [217, 120, 87],
    codex: [15, 163, 128],
    grok: [255, 255, 255],
    calendar: [10, 132, 255],
};

const AGENT_APPS = {claude: 'Claude Code', codex: 'Codex', grok: 'Grok'};
const AGENT_NAMES = {claude: 'Claude', codex: 'Codex', grok: 'Grok'};
const agentIcon = provider => provider === 'codex' ? 'isle-openai' : provider === 'grok' ? 'isle-grok' : 'isle-claude';
const agentTone = provider => provider === 'codex' ? 'codex' : 'claude';

/** The badge's gradient: the tone's colour, brighter at the top (white, fainter, for a neutral one). */
function badgeStyle(rgb, neutral) {
    const [top, bottom] = neutral ? [0.16, 0.08] : [0.34, 0.16];
    const base = neutral ? [255, 255, 255] : rgb;
    return `linear-gradient(to bottom, rgba(${base}, ${top}), rgba(${base}, ${bottom}))`;
}

/** A focus session or a break ran out (alerts.js `pomodoroFinished`). */
export function pomodoroFinished(finished, next, state, start) {
    const waiting = !state?.running;
    const minutes = state ? Math.round(state.duration_ms / 60000) : 0;
    if (finished === 'focus') {
        return {
            key: 'pomodoro',
            icon: 'isle-timer-symbolic',
            title: 'Focus session done',
            subtitle: waiting ? 'Take a break when you’re ready' : `Enjoy a ${minutes}-minute break`,
            tone: 'good',
            action: waiting ? {label: 'Start break', callback: start} : null,
            duration: REMINDER_DURATION,
            page: 'focus',
        };
    }
    const names = {focus: 'Focus', short_break: 'Break', long_break: 'Long break'};
    return {
        key: 'pomodoro',
        icon: 'isle-timer-symbolic',
        title: next === 'focus' ? 'Break is over' : names[next] ?? 'Timer',
        subtitle: waiting ? 'Ready for another focus session?' : `${minutes} minutes of focus`,
        tone: 'focus',
        action: waiting ? {label: 'Start focus', callback: start} : null,
        duration: REMINDER_DURATION,
        page: 'focus',
    };
}

/** "5-hour limit", "Weekly limit", "Weekly Opus limit" (alerts.js `limitName`). */
function limitName(window) {
    if (window === '5 hours')
        return '5-hour limit';
    const [span, model] = String(window).split(' · ');
    const kind = span === 'Week' ? 'Weekly' : span;
    return model ? `${kind} ${model} limit` : `${kind} limit`;
}

/** An AI plan limit nears its end (alerts.js `usage`): the provider's logo in a ring of the share, "86%" beside it. */
export function usageAlert(provider, window, percent, resetsAt, critical) {
    const at = new Date(resetsAt);
    // Within the next 20 hours (later today or tomorrow morning), the time alone; later, with the day.
    const when = resetsAt - Date.now() < 20 * 3600e3 ? timeOfDay(at)
        : `${at.toLocaleDateString('en-GB', {weekday: 'short'})} ${timeOfDay(at)}`;
    const name = provider.toLowerCase();
    const kind = name.includes('claude') ? 'claude' : name.includes('grok') ? 'grok'
        : name.includes('gpt') || name.includes('codex') ? 'codex' : null;
    return {
        key: `usage:${provider}:${window}`,
        icon: {claude: 'isle-claude', grok: 'isle-grok', codex: 'isle-openai'}[kind] ?? 'isle-gauge',
        ring: Math.min(1, percent / 100),
        title: provider,
        subtitle: resetsAt > 0 ? `${limitName(window)} · resets ${when}` : limitName(window),
        value: `${Math.round(percent)}%`,
        // In the plan's own colour, as on the Usage page; red once nearly used up.
        tone: Math.round(percent) >= critical ? 'bad' : kind ?? 'neutral',
        duration: REMINDER_DURATION,
        page: 'usage',
    };
}

/** The service a meeting link joins on, by name. */
function meetingService(link) {
    const host = (/^https:\/\/([^/]+)/.exec(link)?.[1] ?? '').toLowerCase();
    if (host === 'meet.google.com')
        return 'Google Meet';
    if (host.endsWith('zoom.us'))
        return 'Zoom';
    if (host.startsWith('teams.'))
        return 'Microsoft Teams';
    if (host.endsWith('webex.com'))
        return 'Webex';
    return null;
}

/** An event starts soon (alerts.js `meeting`): its name and minutes left, its times, and Join for a meeting link. */
export function meetingAlert(event, join) {
    const start = new Date(event.start_ms);
    const minutes = Math.max(0, Math.round((event.start_ms - Date.now()) / 60000));
    const span = event.end_ms ? `${timeOfDay(start)} – ${timeOfDay(new Date(event.end_ms))}` : timeOfDay(start);
    const service = event.link ? meetingService(event.link) : null;
    return {
        key: `meeting:${event.id ?? event.title}:${event.start_ms}`,
        icon: event.link ? 'isle-video-symbolic' : 'isle-calendar',
        title: minutes > 0 ? `${event.title} in ${minutes} min` : `${event.title} now`,
        subtitle: service ? `${span} · ${service}` : span,
        tone: 'calendar',
        duration: REMINDER_DURATION,
        action: event.link ? {label: 'Join', callback: join, primary: true} : null,
        page: 'calendar',
    };
}

/** An app's notification (alerts.js `notification`): its icon, who over what (two lines), opening the app on a click. */
export function notificationAlert(note, open) {
    return {
        key: `notification:${note.id}`,
        id: note.id,
        image: note.icon || null,
        icon: 'isle-bell',
        title: note.title.replace(/\s+/g, ' ').trim(),
        subtitle: note.body.replace(/\s+/g, ' ').trim(),
        subtitleLines: 2,
        duration: 4000,
        page: 'notifications',
        onActivate: open,
    };
}

/** A new song starts (alerts.js `track`): its cover, its title over the artist. `art` is a promise of the cover. */
export function trackAlert(nowPlaying, art) {
    return {
        key: 'track',
        art,
        title: nowPlaying.title,
        subtitle: nowPlaying.artist || nowPlaying.identity,
        duration: STATUS_DURATION,
        page: 'music',
    };
}

/** "npm run build" for the shell; "Edit src/app.js" for another tool (alerts.js `toolLine`). */
function toolLine(tool, input) {
    const line = String(input ?? '').split('\n')[0];
    if (/^(bash|shell|powershell|exec_command|local_shell)$/i.test(tool ?? '') && line)
        return line;
    return [tool, line].filter(Boolean).join(' ');
}

/** An AI asks permission for a tool (alerts.js `agentAsks`): Deny, Always (when Claude offers to keep it), Allow. */
export function agentAsks(request, answer) {
    return {
        key: `agent-ask:${request.id}`,
        id: request.id,
        icon: agentIcon(request.provider),
        title: `${request.project || AGENT_APPS[request.provider]} needs permission`,
        subtitle: request.project ? AGENT_APPS[request.provider] : '',
        code: toolLine(request.tool, request.input),
        tone: 'warn',
        actions: [
            {label: 'Deny', answer: 'deny', callback: () => answer('deny')},
            ...request.always ? [{label: 'Always', answer: 'allow', done: 'Always allowed here', callback: () => answer('always')}] : [],
            {label: 'Allow', answer: 'allow', primary: true, callback: () => answer('allow')},
        ],
        // It waits on the user: it stays until answered, or until the AI stops asking.
        sticky: true,
        duration: 600000,
    };
}

/** The first sentence of what the AI said, plain, at most 140 characters. */
function firstSentence(text) {
    const line = String(text ?? '').split('\n').map(each => each.trim())
        .find(each => each && !/^(#{1,6}\s|([-*_]\s*){3,}$|```)/.test(each)) ?? '';
    const first = line.split(/(?<=[.!?])\s/)[0]
        .replace(/^[#>\s]+/, '')
        .replace(/\*\*([^*]+)\*\*/g, '$1')
        .replace(/`([^`]+)`/g, '$1')
        .trim();
    return first.length > 140 ? `${first.slice(0, 139).trimEnd()}…` : first;
}

/** An AI needs the user, waits, or is done (alerts.js `agent` and `agentDone`). */
export function agentEvent(event) {
    const name = AGENT_NAMES[event.provider] ?? 'Your AI';
    if (event.kind === 'done') {
        return {
            key: `agent:${event.provider}:${event.session || event.project}`,
            icon: 'isle-check-symbolic',
            title: firstSentence(event.message) || 'Finished',
            titleLines: 2,
            subtitle: `${[event.project, AGENT_APPS[event.provider]].filter(Boolean).join(' · ')} finished`,
            tone: 'good',
            duration: REMINDER_DURATION,
        };
    }
    const tool = /permission to use (.+?)\.?$/i.exec(event.message ?? '')?.[1];
    const detail = event.kind === 'permission' && tool ? `Allow ${tool}?` : '';
    return {
        key: `agent:${event.provider}:${event.session || event.project}`,
        icon: agentIcon(event.provider),
        title: event.kind === 'permission' ? `${name} needs your OK` : `${name} is waiting for you`,
        subtitle: [event.project, detail].filter(Boolean).join(' · '),
        tone: agentTone(event.provider),
        duration: REMINDER_DURATION + 4000,
        sticky: true,
    };
}

export class PeekView {
    constructor() {
        this.node = el('div', 'isle-view isle-peek');
        this._badge = el('div', 'isle-peek-icon');
        this._icon = icon('isle-timer-symbolic', 16);
        this._badge.append(this._icon);
        // A share as a ring round a lit badge; a song's cover.
        this._ringIcon = icon('isle-gauge', 12);
        this._ringBadge = el('div', 'isle-peek-ring-badge', [this._ringIcon]);
        this._ring = new Ring({size: 32, lineWidth: 3, child: this._ringBadge});
        this._art = new Art({size: 32, radius: 8});
        // An app's own icon stands alone, without a badge.
        this._image = el('img', 'isle-peek-image');
        this._lead = el('div', 'isle-peek-lead', [this._badge]);
        this._value = el('div', 'isle-peek-value');
        this._title = el('div', 'isle-peek-title');
        this._subtitle = el('div', 'isle-peek-subtitle');
        this._code = el('div', 'isle-peek-code');
        this._actions = el('div', 'isle-peek-actions');
        this._texts = el('div', 'isle-peek-texts', [this._title, this._subtitle, this._code, this._actions]);
        this._accessory = el('div', 'isle-peek-accessory');
        this._row = el('div', 'isle-peek-row', [this._lead, this._texts, this._accessory]);
        this.node.append(this._row);
        this.alert = null;
        /** Called when a plain alert is clicked (not its button), and when it is done with. */
        this.onOpen = null;
        this.onDone = null;
        this.onResize = null;
        this.node.addEventListener('click', () => {
            if (!this.alert || this.alert.actions)
                return;
            // A notification opens its app; the others their page.
            if (this.alert.onActivate) {
                this.alert.onActivate();
                this.onDone?.(this.alert);
            } else {
                this.onOpen?.(this.alert);
            }
        });
    }

    get preferredWidth() {
        return this.alert?.actions ? Math.ceil(this._row.offsetWidth) : Math.min(MAX_WIDTH, Math.ceil(this._row.scrollWidth));
    }

    get height() {
        return this.alert?.actions || this.alert?.titleLines || this.alert?.subtitleLines
            ? Math.max(PEEK_HEIGHT, Math.ceil(this._row.offsetHeight)) : PEEK_HEIGHT;
    }

    show(alert) {
        const fresh = alert.key !== this.alert?.key || alert.title !== this.alert?.title;
        this.alert = alert;
        const asks = Boolean(alert.actions);
        this.node.classList.toggle('asks', asks);
        this.node.classList.toggle('two-lines', Boolean(alert.titleLines));
        this.node.classList.toggle('subtitle-lines', Boolean(alert.subtitleLines));
        const rgb = TONES[alert.tone ?? 'neutral'] ?? TONES.neutral;
        const neutral = (alert.tone ?? 'neutral') === 'neutral';
        let moving = this._badge;
        if (alert.image) {
            this._lead.replaceChildren(this._image);
            this._image.src = alert.image;
        } else if (alert.art !== undefined) {
            this._lead.replaceChildren(this._art.node);
            if (fresh)
                this._art.url = null;
            Promise.resolve(alert.art).then(url => {
                if (this.alert === alert)
                    this._art.url = url;
            });
            moving = null;
        } else if (alert.ring !== undefined) {
            this._lead.replaceChildren(this._ring.node);
            this._ringBadge.style.background = badgeStyle(rgb, neutral);
            this._ringIcon.style.color = `rgb(${rgb})`;
            this._ringIcon.style.setProperty('--icon', `url("icons/${alert.icon}.svg")`);
            if (fresh)
                this._sweep(alert, `rgb(${rgb})`);
            moving = this._ringBadge;
        } else {
            this._lead.replaceChildren(this._badge);
            this._badge.style.background = badgeStyle(rgb, neutral);
            this._icon.style.color = `rgb(${rgb})`;
            this._icon.style.setProperty('--icon', `url("icons/${alert.icon}.svg")`);
        }
        this._title.textContent = alert.title;
        this._subtitle.textContent = alert.subtitle ?? '';
        this._subtitle.style.display = alert.subtitle ? '' : 'none';
        this._code.textContent = alert.code ?? '';
        this._code.style.display = alert.code ? '' : 'none';
        this._accessory.replaceChildren();
        this._actions.replaceChildren();
        if (alert.value) {
            this._value.style.color = neutral ? '' : `rgb(${rgb})`;
            if (fresh || alert.ring === undefined)
                this._value.textContent = alert.value;
            this._accessory.append(this._value);
        }
        if (alert.action) {
            const button = el('button', `isle-pill isle-pill-small${alert.action.primary ? ' isle-pill-white' : ''}`, [alert.action.label]);
            button.addEventListener('click', event => {
                event.stopPropagation();
                alert.action.callback();
                this.onDone?.(alert);
            });
            this._accessory.append(button);
        }
        for (const action of alert.actions ?? []) {
            const button = el('button', `isle-pill isle-pill-small${action.primary ? ' isle-pill-white' : ''}`, [action.label]);
            button.addEventListener('click', event => {
                event.stopPropagation();
                this._answered(alert, action);
            });
            this._actions.append(button);
        }
        // Its icon moves once, as a new alert comes, once the island has made room.
        if (fresh && moving) {
            moving.classList.remove('bounce');
            setTimeout(() => moving.classList.add('bounce'), RING_DELAY);
        }
    }

    /** The ring fills to its share once the island has made room, the value counting up with it. */
    _sweep(alert, color) {
        cancelAnimationFrame(this._sweeping);
        const final = alert.ring;
        const digits = Number.parseInt(alert.value, 10);
        const suffix = Number.isNaN(digits) ? '' : alert.value.slice(String(digits).length);
        this._ring.set(0, color);
        if (!Number.isNaN(digits))
            this._value.textContent = `0${suffix}`;
        const start = performance.now() + RING_DELAY;
        const frame = now => {
            if (this.alert !== alert)
                return;
            const t = Math.min(1, Math.max(0, (now - start) / RING_SWEEP));
            const eased = 1 - (1 - t) ** 3;
            this._ring.set(final * eased, color);
            if (!Number.isNaN(digits))
                this._value.textContent = `${Math.round(digits * eased)}${suffix}`;
            if (t < 1)
                this._sweeping = requestAnimationFrame(frame);
        };
        this._sweeping = requestAnimationFrame(frame);
    }

    /** A button of an alert that asks: its answer goes at once, and says so a moment before the alert goes. */
    _answered(alert, action) {
        action.callback();
        const allowed = action.answer === 'allow';
        const said = el('div', `isle-peek-answered ${allowed ? 'isle-peek-allowed' : 'isle-peek-denied'}`,
            [action.done ?? (allowed ? 'Allowed' : 'Denied')]);
        this._actions.replaceWith(said);
        setTimeout(() => {
            said.replaceWith(this._actions);
            if (this.alert === alert)
                this.onDone?.(alert);
        }, ANSWERED_MS);
    }
}
