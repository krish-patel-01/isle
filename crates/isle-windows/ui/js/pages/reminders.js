// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The Reminders page (gnome-extension/src/pages/tasks.js): an orange title and the count of open ones, a ring and
// a hairline per reminder, and a last row to add one. A ticked one is struck through, lingers, then goes.

import {daemon} from '../daemon.js';
import {el, iconButton} from '../widgets.js';

/** A ticked reminder stays this long (struck through) before it goes. */
const LINGER_MS = 900;
const LEAVE_MS = 260;

export class RemindersPage {
    constructor() {
        this.name = 'reminders';
        this.node = el('div', 'isle-page isle-reminders');
        this._count = el('span', 'isle-reminders-count');
        const add = iconButton('isle-plus-symbolic', 'isle-button-round isle-button-small isle-reminders-add',
            () => this._entry.focus(), {size: 12, label: 'New reminder'});
        const header = el('div', 'isle-reminders-header', [el('span', 'isle-reminders-title', ['Reminders']), this._count, add]);

        this._list = el('div', 'isle-reminder-list');
        this._entry = el('input', 'isle-reminder-field');
        this._entry.placeholder = 'New reminder';
        this._entry.addEventListener('focus', () => window.isleKeyboard?.(true));
        this._entry.addEventListener('blur', () => window.isleKeyboard?.(false));
        this._entry.addEventListener('keydown', event => {
            if (event.key === 'Enter' && this._entry.value.trim()) {
                daemon.call('todo', 'Add', {title: this._entry.value});
                this._entry.value = '';
            } else if (event.key === 'Escape') {
                this._entry.value = '';
                this._entry.blur();
            }
        });
        const newRow = el('div', 'isle-reminder isle-reminder-new', [
            el('div', 'isle-reminder-check faint'),
            el('div', 'isle-reminder-body', [el('div', 'isle-reminder-content', [this._entry])]),
        ]);
        this._empty = el('div', 'isle-reminders-empty', ['No reminders']);
        this._scroll = el('div', 'isle-reminder-scroll', [this._list, newRow]);
        this.node.append(header, this._scroll);
        /** Ticked rows on their way out: id → timer. */
        this._leaving = new Map();
        daemon.connect(() => this._update(), 'todo');
        this._update();
    }

    get preferredWidth() {
        return null;
    }

    _update() {
        const items = daemon.state.todo ?? [];
        const open = items.filter(item => !item.done || this._leaving.has(item.id));
        this._count.textContent = String(items.filter(item => !item.done).length);
        this._list.replaceChildren(...open.map(item => this._row(item)));
    }

    _row(task) {
        const check = el('button', `isle-reminder-check${task.done ? ' checked' : ''}`, [el('div', 'isle-reminder-dot')]);
        check.setAttribute('aria-label', task.done ? 'Not done' : 'Done');
        const title = el('div', `isle-reminder-title${task.done ? ' done' : ''}`, [task.title]);
        const titleBox = el('div', 'isle-reminder-title-box', [title, el('div', 'isle-reminder-strike')]);
        const remove = iconButton('isle-close-symbolic', 'isle-button-flat isle-reminder-remove',
            () => daemon.call('todo', 'Remove', {id: task.id}), {size: 12, label: 'Remove'});
        const content = el('div', 'isle-reminder-content', [titleBox, remove]);
        const row = el('div', 'isle-reminder', [check, el('div', 'isle-reminder-body', [content, el('div', 'isle-reminder-line')])]);

        check.addEventListener('click', () => {
            const done = !task.done;
            if (done) {
                const timer = setTimeout(() => {
                    row.classList.add('leaving');
                    setTimeout(() => {
                        this._leaving.delete(task.id);
                        this._update();
                    }, LEAVE_MS);
                }, LINGER_MS);
                this._leaving.set(task.id, timer);
            } else {
                clearTimeout(this._leaving.get(task.id));
                this._leaving.delete(task.id);
            }
            daemon.call('todo', 'SetDone', {id: task.id, done});
        });
        // Click the title to rename it in place.
        title.addEventListener('click', () => {
            const field = el('input', 'isle-reminder-field');
            field.value = task.title;
            titleBox.replaceChildren(field);
            field.focus();
            field.select();
            const finish = save => {
                if (save && field.value.trim() && field.value !== task.title)
                    daemon.call('todo', 'Rename', {id: task.id, title: field.value});
                else
                    this._update();
            };
            field.addEventListener('focus', () => window.isleKeyboard?.(true));
            window.isleKeyboard?.(true);
            field.addEventListener('blur', () => {
                window.isleKeyboard?.(false);
                finish(true);
            });
            field.addEventListener('keydown', event => {
                if (event.key === 'Enter')
                    field.blur();
                else if (event.key === 'Escape') {
                    field.value = task.title;
                    field.blur();
                }
            });
        });
        return row;
    }
}
