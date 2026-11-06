// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The Assistant page (gnome-extension/src/chat.js): the day's conversation, what the assistant is doing as it
// works, the command it asks to run with Run and Deny, and a field to type a request with the microphone beside it.

import {daemon} from '../daemon.js';
import {el, icon, iconButton, setButtonIcon} from '../widgets.js';

const {invoke} = window.__TAURI__.core;

/** What the assistant is doing, as said while it works. */
const WORKING = {listening: 'Listening…', transcribing: 'Working it out…', thinking: 'Thinking…', acting: 'Working…'};

/** Text as the assistant writes it: paragraphs, "- " lists, **bold** and `code`; nothing else is markup. */
export function rich(text) {
    const node = el('div', 'isle-chat-rich');
    let list = null;
    for (const line of String(text ?? '').split('\n')) {
        const item = /^\s*[-*•]\s+(.*)$/.exec(line);
        if (item) {
            if (!list) {
                list = el('ul');
                node.append(list);
            }
            list.append(inline(el('li'), item[1]));
            continue;
        }
        list = null;
        if (line.trim())
            node.append(inline(el('p'), line));
    }
    return node;
}

function inline(node, text) {
    for (const part of text.split(/(\*\*[^*]+\*\*|`[^`]+`)/)) {
        if (part.startsWith('**') && part.endsWith('**') && part.length > 4)
            node.append(el('strong', '', [part.slice(2, -2)]));
        else if (part.startsWith('`') && part.endsWith('`') && part.length > 2)
            node.append(el('code', '', [part.slice(1, -1)]));
        else if (part)
            node.append(document.createTextNode(part));
    }
    return node;
}

export class AssistantPage {
    constructor() {
        this.name = 'assistant';
        this.node = el('div', 'isle-page isle-assistant');
        const fresh = iconButton('isle-plus-symbolic', 'isle-button-round isle-button-small', () => invoke('assistant_new_chat'),
            {size: 12, label: 'New chat'});
        this.node.append(el('div', 'isle-assistant-header', [el('span', 'isle-assistant-title', ['Assistant']), fresh]));
        this._list = el('div', 'isle-chat-list');
        this._scroll = el('div', 'isle-chat-scroll', [this._list]);
        this._field = el('input', 'isle-chat-field');
        this._field.placeholder = 'Ask anything';
        this._field.addEventListener('focus', () => window.isleKeyboard?.(true));
        this._field.addEventListener('blur', () => window.isleKeyboard?.(false));
        this._field.addEventListener('keydown', event => {
            if (event.key === 'Enter' && this._field.value.trim()) {
                invoke('assistant_ask', {text: this._field.value.trim()});
                this._field.value = '';
            } else if (event.key === 'Escape') {
                this._field.value = '';
                this._field.blur();
            }
        });
        this._mic = iconButton('isle-mic-symbolic', 'isle-button-round isle-chat-mic', () => {
            if (this._busy)
                invoke('assistant_stop');
            else
                invoke('assistant_listen');
        }, {size: 14, label: 'Speak'});
        this.node.append(this._scroll, el('div', 'isle-chat-entry', [this._field, this._mic]));
        daemon.connect(() => this._update(), 'assistantChat');
        daemon.connect(() => this._update(), 'assistant');
        this._update();
    }

    get preferredWidth() {
        return null;
    }

    setActive(active) {
        if (active)
            requestAnimationFrame(() => (this._scroll.scrollTop = this._scroll.scrollHeight));
    }

    _update() {
        const state = daemon.state.assistant ?? {phase: 'idle'};
        const items = daemon.state.assistantChat ?? [];
        this._busy = Boolean(WORKING[state.phase]);
        setButtonIcon(this._mic, this._busy ? 'isle-stop-symbolic' : 'isle-mic-symbolic');
        this._mic.classList.toggle('busy', this._busy);
        const rows = items.map(item => item.role === 'you'
            ? el('div', 'isle-chat-you', [item.text])
            : el('div', 'isle-chat-isle', [rich(item.text)]));
        if (this._busy) {
            const said = state.draft || state.activity || WORKING[state.phase];
            rows.push(el('div', `isle-chat-isle working${state.draft ? '' : ' dim'}`, [state.draft ? rich(said) : said]));
        }
        if (state.phase === 'error' && state.error)
            rows.push(el('div', 'isle-chat-error', [icon('isle-warning-symbolic', 12), el('span', '', [state.error])]));
        if (state.card)
            rows.push(this._card(state.card));
        if (!rows.length) {
            rows.push(el('div', 'isle-chat-empty', [
                el('div', 'isle-chat-empty-title', ['Ask me anything']),
                el('div', '', ['Type below, or press Win+Alt+I and speak. I can open and arrange apps, find files, ' +
                    'change settings, look into problems and install things, with your OK.']),
            ]));
        }
        const atEnd = this._scroll.scrollHeight - this._scroll.scrollTop - this._scroll.clientHeight < 40;
        this._list.replaceChildren(...rows);
        if (atEnd)
            this._scroll.scrollTop = this._scroll.scrollHeight;
    }

    _card(card) {
        const parts = [el('div', 'isle-chat-card-title', [card.why])];
        if (card.detail)
            parts.push(el('div', 'isle-chat-card-detail', [card.detail]));
        parts.push(el('code', 'isle-chat-card-code', [card.command]));
        if (card.running) {
            parts.push(el('div', 'isle-chat-card-running', ['Running…']));
        } else {
            const deny = el('button', 'isle-pill isle-pill-small', ['Deny']);
            deny.addEventListener('click', () => invoke('assistant_answer', {id: card.id, run: false}));
            const run = el('button', 'isle-pill isle-pill-small isle-pill-white', [card.root ? 'Run as admin' : 'Run']);
            run.addEventListener('click', () => invoke('assistant_answer', {id: card.id, run: true}));
            parts.push(el('div', 'isle-chat-card-actions', [deny, run]));
        }
        return el('div', 'isle-chat-card', parts);
    }
}
