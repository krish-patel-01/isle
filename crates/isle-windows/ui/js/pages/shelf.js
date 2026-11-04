// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The Shelf page (gnome-extension/src/pages/shelf.js and clips.js). Clipboard: the history as cards, pinned first,
// then newest. Click a card to copy it; on hover: pin, a hand-off (text to Reminders, pictures and files to Files)
// and ×. Filters by kind, a search that opens into a field (← → move between cards, Enter pastes into the window
// behind), and Clear all. Files: files dropped on the island (files.js). Phone, while sharing: a QR code to scan
// (phone.js).

import {daemon} from '../daemon.js';
import {el, icon, iconButton} from '../widgets.js';
import {FilesView, SHELF_CAPACITY} from './files.js';
import {ago} from './notifications.js';
import {PhoneCode, PhoneView} from './phone.js';

// Phone has a tab only while sharing.
const TABS = [
    {name: 'clipboard', label: 'Clipboard'},
    {name: 'files', label: 'Files'},
    {name: 'phone', label: 'Phone'},
];
/** How long the hint says a file was added, or why sharing could not start. */
const ADDED_MS = 2200;
const SHARE_ERROR_MS = 5000;

const {invoke} = window.__TAURI__.core;

const FILTERS = [
    {kind: 'text', icon: 'isle-text', name: 'Text'},
    {kind: 'link', icon: 'isle-link', name: 'Links'},
    {kind: 'image', icon: 'isle-image', name: 'Images'},
    {kind: 'files', icon: 'isle-file', name: 'Files'},
];
/** Isle keeps 20 recent items and up to 10 pinned ones. */
const MAX_CARDS = 30;
const MAX_PINNED = 10;
/** How long "Clear all" waits for its confirming click. */
const CONFIRM_MS = 3000;
/** How long "Copied" and friends stay on a card. */
const FEEDBACK_MS = 900;
/** Looks like code: braces, semicolons at line ends, arrows, tags, indents. */
const CODE_PATTERN = /[{};]\s*$|=>|<\/?[a-z][\w-]*>|^\s{2,}\S|\b(function|const|def|class|import|return)\b/m;
/** The card's picture in CSS px; thumbnails are made at twice that or smaller (clipRules.js). */
const CARD_PICTURE = [136, 108];
const THUMB_SCALE = 2;

/** The `background-size` for a `width`×`height` px thumbnail at `density` screen pixels per CSS px:
 * `cover` when it has the pixels for that, else one of its pixels per screen pixel, never blown up. */
export function thumbSize(width, height, density) {
    if (!(width > 0 && height > 0))
        return 'cover';
    const scale = Math.min(Math.max(density, 1), THUMB_SCALE);
    const cover = Math.max(CARD_PICTURE[0] / width, CARD_PICTURE[1] / height);
    if (cover * scale <= 1 + 1e-6)
        return 'cover';
    return width >= height ? `${Math.round(width / scale)}px` : `auto ${Math.round(height / scale)}px`;
}

/** Icons of programs and files by path, asked of Windows once. */
const icons = new Map();
function iconOf(path) {
    if (!icons.has(path))
        icons.set(path, invoke('file_icon', {path}).catch(() => ''));
    return icons.get(path);
}

/** An image filled in with `path`'s icon once Windows gives it; `fallback` (an Isle icon) until then or without. */
function pathIcon(path, size, className, fallback = null) {
    const box = el('span', className);
    if (fallback)
        box.append(icon(fallback, size));
    iconOf(path).then(url => {
        if (!url)
            return;
        const image = el('img');
        image.src = url;
        image.width = image.height = size;
        box.replaceChildren(image);
    });
    return box;
}

/** A link card's site name; the whole URL if it doesn't parse. */
function hostOf(url) {
    try {
        return new URL(url).host || url;
    } catch {
        return url;
    }
}

export class ShelfPage {
    constructor() {
        this.name = 'shelf';
        this.node = el('div', 'isle-page isle-shelf');
        this.onClose = null;
        this._kind = null;
        this._query = '';
        /** The items Isle found holding the words in their whole text (cards show only its start). */
        this._found = null;
        this._searches = 0;
        this._selected = 0;
        this._keyboard = false;
        this._all = [];
        this._shown = [];
        /** Drawn cards by item id. */
        this._cards = new Map();
        /** Items copied while the page is open; moved to the front on leaving. */
        this._copied = [];
        this._flashing = null;
        this._active = false;
        this._tab = 'clipboard';
        /** A file is being dragged over the island, and over the drop zone. */
        this._dropping = false;
        this._dropOver = false;
        this._hovered = null;
        this._added = false;
        this._note = null;
        this._phoneSharing = false;

        this._files = new FilesView();
        this._files.onNote = text => this._showNote(text);
        this._phone = new PhoneView();
        this._phone.onAnswer = (id, allow) => daemon.call('Share1', 'Answer', {id, allow});
        this._phone.onRemove = id => daemon.call('Share1', 'Remove', {id});
        this._code = new PhoneCode();

        this._column = el('div', 'isle-shelf-column', [this._buildBar()]);
        this.node.append(this._column, this._code.node);
        this._strip = el('div', 'isle-clips');
        this._scroll = el('div', 'isle-clips-scroll', [this._strip]);
        // The wheel scrolls the cards sideways, rather than turning the page.
        this._scroll.addEventListener('wheel', event => {
            if (this._scroll.scrollWidth <= this._scroll.clientWidth)
                return;
            event.preventDefault();
            event.stopPropagation();
            const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
            this._scroll.scrollLeft += delta;
        }, {passive: false});
        this._emptyIcon = el('span');
        this._emptyTitle = el('div', 'isle-empty-title');
        this._emptyHint = el('div', 'isle-empty-hint');
        this._empty = el('div', 'isle-empty', [this._emptyIcon, this._emptyTitle, this._emptyHint]);
        this._clips = el('div', 'isle-shelf-clips', [this._scroll, this._empty]);
        this._column.append(el('div', 'isle-shelf-content', [this._clips, this._files.node, this._phone.node]));

        daemon.connect(() => this._update(), 'clipboard');
        daemon.connect(() => this._update(), 'settings');
        daemon.connect(() => {
            this._noticeNewFiles();
            this._files.update();
            this._syncRefusal();
        }, 'shelf');
        daemon.connect(() => this._syncShare(), 'share');
        daemon.connect((_key, name) => this._showNote(`Received ${name}`), 'shareReceived');
        this._update();
        this._syncShare();
        this.showTab('clipboard');
    }

    // Tabs

    /** Switches to tab `name`; the Phone tab only while sharing. */
    showTab(name) {
        if (name === 'phone' && !this._phoneSharing)
            name = 'files';
        this._tab = name;
        for (const [tab, button] of this._tabButtons)
            button.classList.toggle('checked', tab === name);
        this._clips.hidden = name !== 'clipboard';
        this._files.node.hidden = name !== 'files';
        this._phone.node.hidden = name !== 'phone';
        this._syncControls();
        if (name !== 'files')
            this._setAdded(false);
    }

    /** Each tab's controls in the bar, and the phone's code beside the Phone tab. */
    _syncControls() {
        const name = this._tab;
        this._clipboardControls.hidden = name !== 'clipboard';
        this._filesHint.hidden = name !== 'files';
        this._phoneStart.hidden = name !== 'files' || this._phoneSharing;
        this._phoneControls.hidden = name !== 'phone';
        this._code.node.hidden = name !== 'phone';
        this._updateFilesHint();
    }

    // Files

    /** A file is being dragged over the island: say whether it can land. */
    set dropping(dropping) {
        this._dropping = dropping;
        this._files.dropping = dropping;
        if (!dropping) {
            this._dropOver = this._files.dropOver = false;
            this._hovered = null;
        }
        this._syncRefusal();
    }

    /** The files on their way (paths), and where the pointer is (CSS px in the window). */
    dragOver(paths, x, y) {
        if (paths?.length)
            this._hovered = paths;
        this._dropOver = this._dropping && this._files.contains(x, y);
        this._files.dropOver = this._dropOver;
        this._syncRefusal();
    }

    /** The drag ended over the island: adds the files if let go over the zone. Returns whether they were turned
     * away (the island then shakes its head). */
    async drop(paths, x, y) {
        const over = this._tab === 'files' && this._files.contains(x, y);
        const refused = over && Boolean(this._refusal);
        this.dropping = false;
        if (!over || refused || !paths?.length)
            return refused;
        const result = await invoke('shelf_add', {paths}).catch(() => null);
        if (result?.added)
            this._files.flashLanded();
        if (result?.refused) {
            this._showNote(result.refused === 'room' ? `Full · only ${SHELF_CAPACITY} fit` : 'Only files on this computer');
            return true;
        }
        return false;
    }

    /** Why the files on their way can't all land: 'room' (more than fit) or null. Files are local on Windows. */
    get _refusal() {
        if (!this._dropping)
            return null;
        const shelf = this._files.items;
        const fresh = this._hovered ? this._hovered.filter(path => !shelf.some(item => item.path === path)).length : 1;
        return fresh > 0 && shelf.length + fresh > SHELF_CAPACITY ? 'room' : null;
    }

    _syncRefusal() {
        this._files.refusal = this._refusal;
        this._updateFilesHint();
    }

    /** A file just landed: the hint says so a moment. */
    _noticeNewFiles() {
        const uris = (daemon.state.shelf ?? []).map(item => item.uri);
        const known = this._shelved;
        this._shelved = uris;
        if (known && uris.some(uri => !known.includes(uri)) && this._tab === 'files') {
            this._setAdded(true);
            clearTimeout(this._addedTimer);
            this._addedTimer = setTimeout(() => this._setAdded(false), ADDED_MS);
        }
    }

    _setAdded(added) {
        this._added = added;
        this._updateFilesHint();
    }

    /** A line for the files hint for a moment (saved, received, couldn't share). */
    _showNote(text) {
        this._note = text;
        this._updateFilesHint();
        clearTimeout(this._noteTimer);
        this._noteTimer = setTimeout(() => {
            this._note = null;
            this._updateFilesHint();
        }, SHARE_ERROR_MS);
    }

    /** Says what the Files tab is for, and when it is full. */
    _updateFilesHint() {
        const count = this._files.items.length;
        const full = count >= SHELF_CAPACITY;
        const refusal = this._refusal;
        let text;
        if (this._note && !this._dropping)
            text = this._note;
        else if (count === 0 && !this._added)
            text = '';
        else if (refusal && full)
            text = 'Full · remove a file first';
        else if (refusal)
            text = `Only ${SHELF_CAPACITY - count} more fit`;
        else if (this._dropping)
            text = this._dropOver ? 'Release to add it' : 'Drop to keep it on the shelf';
        else if (this._added)
            text = 'Added · click to copy, or drag it into any app';
        else if (full)
            text = 'Full · remove a file to add another';
        else
            text = this._files.items.some(item => item.own)
                ? `${count} of ${SHELF_CAPACITY} · click to copy, ↓ to save, or drag out`
                : `${count} of ${SHELF_CAPACITY} · click to copy, or drag into any app`;
        this._filesHint.textContent = text;
        this._filesHint.classList.toggle('active', this._dropping || this._added);
    }

    // Phone

    /** Starts sharing with a phone; the Phone tab opens once Isle says it shares. */
    async _startSharing() {
        this._startingShare = true;
        this._phoneStart.disabled = true;
        try {
            await invoke('share_start');
        } catch (err) {
            this._startingShare = false;
            this._showNote(String(err));
        }
        this._phoneStart.disabled = false;
        if (this._phoneSharing)
            this._openPhoneTab();
    }

    _openPhoneTab() {
        if (!this._startingShare)
            return;
        this._startingShare = false;
        this.showTab('phone');
    }

    /** Follows Isle's sharing: the Phone tab comes and goes with it. */
    async _syncShare() {
        const state = daemon.state.share;
        const sharing = Boolean(state?.active);
        const changed = sharing !== this._phoneSharing;
        this._phoneSharing = sharing;
        this._tabButtons.get('phone').hidden = !sharing;
        if (sharing)
            this._phone.update(state);
        else
            this._phone.reset();
        if (!sharing) {
            this._codeSession = null;
            this._code.rows = [];
            this._phone.url = null;
        } else if (state.session !== this._codeSession) {
            this._codeSession = state.session;
            const code = await invoke('share_code').catch(() => null);
            if (code && this._codeSession === state.session) {
                this._code.rows = code.qr ?? [];
                this._phone.url = code.url ?? null;
            }
        }
        if (sharing && this._startingShare)
            this._openPhoneTab();
        else if (!sharing && this._tab === 'phone')
            this.showTab('files');
        else if (changed)
            this._syncControls();
    }

    get preferredWidth() {
        return null;
    }

    // Building

    /** The tabs, then the controls of the tab shown: filters, search and Clear; the files hint and Phone; Stop. */
    _buildBar() {
        this._tabButtons = new Map();
        const tabs = el('div', 'isle-segmented', TABS.map(({name, label}) => {
            const button = el('button', 'isle-segment', [label]);
            button.addEventListener('click', () => this.showTab(name));
            this._tabButtons.set(name, button);
            return button;
        }));

        this._filterButtons = new Map();
        const filters = el('div', 'isle-segmented isle-segmented-icons', FILTERS.map(filter => {
            const button = el('button', 'isle-segment', [icon(filter.icon, 13)]);
            button.setAttribute('aria-label', `Only ${filter.name.toLowerCase()}`);
            button.title = filter.name;
            button.addEventListener('click', () => this._setFilter(this._kind === filter.kind ? null : filter.kind));
            this._filterButtons.set(filter.kind, button);
            return button;
        }));

        this._search = el('input', 'isle-shelf-field');
        this._search.placeholder = 'Search';
        this._search.hidden = true;
        this._search.addEventListener('input', () => this._searchFor(this._search.value));
        this._search.addEventListener('focus', () => {
            window.isleKeyboard?.(true);
            this._keyboard = true;
            this._syncSelection();
        });
        this._search.addEventListener('blur', () => {
            window.isleKeyboard?.(false);
            this._keyboard = false;
            this._syncSelection();
            if (!this._search.value)
                this._closeSearch();
        });
        this._search.addEventListener('keydown', event => this._onSearchKey(event));
        this._searchBox = el('div', 'isle-entry isle-shelf-search', [icon('isle-search', 12), this._search]);
        this._searchBox.hidden = true;
        this._spacer = el('div', 'isle-shelf-spacer');
        this._searchButton = iconButton('isle-search', 'isle-button-round isle-button-small',
            () => this._openSearch(), {size: 13, label: 'Search the clipboard history'});

        this._clear = iconButton('isle-trash', 'isle-button-round isle-button-small', () => this._askToClear(),
            {size: 14, label: 'Clear the clipboard history'});
        this._confirm = el('button', 'isle-pill isle-pill-small isle-shelf-confirm', ['Clear all']);
        this._confirm.hidden = true;
        this._confirm.addEventListener('click', () => {
            this._endConfirm();
            daemon.call('Clipboard1', 'Clear');
        });

        this._clipboardControls = el('div', 'isle-shelf-controls',
            [filters, this._spacer, this._searchBox, this._searchButton, this._clear, this._confirm]);

        // Files: what the tab is for, and sharing with a phone.
        this._filesHint = el('div', 'isle-shelf-hint');
        this._phoneStart = el('button', 'isle-pill isle-pill-small isle-shelf-phone',
            [icon('isle-smartphone-symbolic', 13), el('span', '', ['Phone'])]);
        this._phoneStart.setAttribute('aria-label', 'Share the Shelf with your phone');
        this._phoneStart.addEventListener('click', () => this._startSharing());

        // Phone: Stop, at the end.
        const stop = el('button', 'isle-pill isle-pill-small', ['Stop']);
        stop.addEventListener('click', () => daemon.call('Share1', 'Stop'));
        this._phoneControls = el('div', 'isle-shelf-controls isle-shelf-end', [stop]);
        return el('div', 'isle-shelf-bar', [tabs, this._clipboardControls, this._filesHint, this._phoneStart, this._phoneControls]);
    }

    // Search

    _openSearch() {
        this._spacer.hidden = this._searchButton.hidden = true;
        this._searchBox.hidden = this._search.hidden = false;
        this._search.focus();
    }

    _closeSearch() {
        this._searchBox.hidden = this._search.hidden = true;
        this._spacer.hidden = this._searchButton.hidden = false;
    }

    /** Asks Isle which items hold the words in their whole text; the cards change once it answers. */
    async _searchFor(text) {
        const query = text.trim().toLowerCase();
        const search = ++this._searches;
        if (!query) {
            [this._query, this._found] = ['', null];
            this._selected = 0;
            this._filter();
            return;
        }
        const ids = await invoke('clipboard_search', {query}).catch(() => null);
        if (search !== this._searches)
            return;
        [this._query, this._found] = [query, ids ? new Set(ids) : null];
        this._selected = 0;
        this._filter();
    }

    /** ← → move between cards, Enter pastes, Esc empties the search before it can close the island. */
    _onSearchKey(event) {
        switch (event.key) {
        case 'ArrowLeft':
            this._moveSelection(-1);
            break;
        case 'ArrowRight':
            this._moveSelection(1);
            break;
        case 'Enter': {
            const item = this._shown[this._selected];
            if (item)
                this._activate(item, true);
            break;
        }
        case 'Escape':
            if (this._search.value) {
                this._search.value = '';
                this._searchFor('');
            } else {
                this.onClose?.();
            }
            break;
        default:
            return;
        }
        event.preventDefault();
    }

    _moveSelection(step) {
        if (this._shown.length === 0)
            return;
        this._selected = Math.min(this._shown.length - 1, Math.max(0, this._selected + step));
        this._syncSelection();
        this._cards.get(this._shown[this._selected].id)?.scrollIntoView({behavior: 'smooth', inline: 'nearest', block: 'nearest'});
    }

    _syncSelection() {
        const selected = this._keyboard ? this._shown[this._selected]?.id : null;
        for (const [id, card] of this._cards)
            card.classList.toggle('selected', id === selected);
    }

    // Updates

    _setFilter(kind) {
        this._kind = kind;
        for (const [filterKind, button] of this._filterButtons) {
            button.classList.toggle('checked', filterKind === kind);
            button.classList.toggle('dimmed', Boolean(kind) && filterKind !== kind);
        }
        this._selected = 0;
        this._filter();
    }

    /** Redraws the cards from the history; unchanged items keep theirs. */
    _update() {
        this._off = daemon.state.settings?.['clipboard-history'] === false;
        const all = daemon.state.clipboard ?? [];
        this._all = [...all.filter(item => item.pinned), ...all.filter(item => !item.pinned)];
        const keys = new Map(this._all.map(item => [item.id, JSON.stringify(item)]));
        for (const [id, card] of this._cards) {
            if (keys.get(id) !== card._key) {
                card.remove();
                this._cards.delete(id);
            }
        }
        this._clear.disabled = !all.some(item => !item.pinned);
        this._filter();
        // A new copy may hold the words searched for too.
        if (this._query)
            this._searchFor(this._search.value);
    }

    /** Shows the cards of the items that match the filter and the search, in the list's order. */
    _filter() {
        const query = this._query;
        this._shown = this._all
            .filter(item => !this._kind || item.kind === this._kind)
            .filter(item => !query || this._found?.has(item.id) ||
                `${item.preview}\n${item.app_name ?? ''}`.toLowerCase().includes(query))
            .slice(0, MAX_CARDS);
        this._selected = Math.min(this._selected, Math.max(0, this._shown.length - 1));
        const cards = this._shown.map(item => {
            let card = this._cards.get(item.id);
            if (!card) {
                card = this._card(item);
                card._key = JSON.stringify(item);
                this._cards.set(item.id, card);
            }
            return card;
        });
        // Only what changed moves, so a card under the pointer stays put.
        const same = cards.length === this._strip.children.length && cards.every((card, i) => this._strip.children[i] === card);
        if (!same)
            this._strip.replaceChildren(...cards);
        this._scroll.hidden = cards.length === 0;
        this._empty.hidden = cards.length > 0;
        this._updateEmpty();
        this._syncSelection();
    }

    /** Why there are no cards: no match, history off, or nothing yet. */
    _updateEmpty() {
        let iconName = 'isle-clipboard', title, hint;
        if (this._query || this._kind) {
            iconName = 'isle-search';
            title = 'Nothing found';
            hint = 'Try other words or another filter';
        } else if (this._off) {
            title = 'Clipboard history is off';
            hint = 'Turn it on in Isle’s settings';
        } else {
            title = 'Nothing copied yet';
            hint = 'Text, links, images and files you copy show up here';
        }
        if (this._emptyIcon._name !== iconName) {
            this._emptyIcon._name = iconName;
            this._emptyIcon.replaceChildren(icon(iconName, 24, 'isle-empty-icon'));
        }
        this._emptyTitle.textContent = title;
        this._emptyHint.textContent = hint;
    }

    /** Moves the cards' ages ("2m") on. */
    _refreshAges() {
        for (const card of this._cards.values())
            card._age.textContent = ago(card._item.at_ms);
    }

    // Cards

    /** Body, hover actions, header and a feedback layer, stacked. */
    _card(item) {
        const card = el('div', `isle-clip isle-clip-${item.kind}`);
        card._item = item;
        card.append(this._body(item), this._actions(item, card), this._header(item, card));
        card._feedbackIcon = el('span');
        card._feedbackLabel = el('div', 'isle-clip-feedback-label');
        card._feedback = el('div', 'isle-clip-feedback', [el('div', 'isle-clip-feedback-box', [card._feedbackIcon, card._feedbackLabel])]);
        card.append(card._feedback);
        card.addEventListener('click', () => {
            this._selected = this._shown.findIndex(shown => shown.id === item.id);
            this._syncSelection();
            this._activate(item, false);
        });
        if (this._flashing?.id === item.id && Date.now() < this._flashing.until)
            this._showFlash(card);
        return card;
    }

    /** Copies the item back to the clipboard, and with `paste` types it into the window behind. */
    async _activate(item, paste) {
        const copied = await invoke('clipboard_copy', {id: item.id, paste}).catch(() => false);
        if (!copied)
            return;
        if (paste) {
            this.onClose?.();
        } else {
            this._flash(item, 'isle-check-symbolic', 'Copied');
            this._copied = [...this._copied.filter(id => id !== item.id), item.id];
        }
    }

    /** Shows "Copied" (or similar) over the card for a moment. */
    _flash(item, iconName, text) {
        this._flashing = {id: item.id, iconName, text, until: Date.now() + FEEDBACK_MS};
        const card = this._cards.get(item.id);
        if (card)
            this._showFlash(card);
    }

    _showFlash(card) {
        const {iconName, text, until} = this._flashing;
        card._feedbackIcon.replaceChildren(icon(iconName, 18));
        card._feedbackLabel.textContent = text;
        card._feedback.classList.add('shown');
        clearTimeout(card._flashTimer);
        card._flashTimer = setTimeout(() => card._feedback.classList.remove('shown'), Math.max(0, until - Date.now()));
    }

    /** Pin, app icon and age in the top-left corner. */
    _header(item, card) {
        const header = el('div', 'isle-clip-header');
        if (item.pinned)
            header.append(icon('isle-pin-fill', 11, 'isle-clip-pin'));
        if (item.app_id) {
            const app = pathIcon(item.app_id, 13, 'isle-clip-app');
            app.title = item.app_name ?? '';
            header.append(app);
        }
        card._age = el('span', 'isle-clip-age', [ago(item.at_ms)]);
        header.append(card._age);
        return header;
    }

    /** Per kind: a thumbnail, a file icon and name, a host and URL, or the text itself. */
    _body(item) {
        switch (item.kind) {
        case 'image': {
            const thumb = el('div', 'isle-clip-thumb');
            if (item.thumb) {
                thumb.style.backgroundImage = `url("${item.thumb}")`;
                thumb.style.backgroundSize = thumbSize(item.thumb_width, item.thumb_height, window.devicePixelRatio || 1);
            }
            const layers = [thumb];
            if (item.width && item.height)
                layers.push(el('div', 'isle-clip-badge', [`${item.width}×${item.height}`]));
            return el('div', 'isle-clip-layers', layers);
        }
        case 'files': {
            const names = item.preview.split('\n');
            const body = el('div', 'isle-clip-body', [
                pathIcon(item.paths?.[0] ?? '', 28, 'isle-clip-file-icon', 'isle-file'),
                el('div', 'isle-clip-file-name', [names[0]]),
            ]);
            if (names.length > 1)
                body.append(el('div', 'isle-clip-subtle', [`+${names.length - 1} more`]));
            return body;
        }
        case 'link': {
            const link = item.preview.trim();
            return el('div', 'isle-clip-body', [
                el('div', 'isle-clip-host', [hostOf(link)]),
                el('div', 'isle-clip-url', [link.replace(/^https?:\/\//i, '')]),
            ]);
        }
        default: {
            const code = CODE_PATTERN.test(item.preview);
            // Leading blank lines and long runs of spaces waste the card.
            const shown = code ? item.preview.replace(/^\s*\n/, '') : item.preview.replace(/\s+/g, ' ').trim();
            return el('div', 'isle-clip-body', [el('div', code ? 'isle-clip-text isle-clip-code' : 'isle-clip-text', [shown.slice(0, 160)])]);
        }
        }
    }

    /** Pin, a hand-off to Reminders (text) and × in the top-right corner, shown on hover. */
    _actions(item, card) {
        const button = (iconName, label, onClick) => {
            const node = iconButton(iconName, 'isle-clip-action', onClick, {size: 11, label});
            // A click on one of these never also copies the card.
            node.addEventListener('click', event => event.stopPropagation());
            return node;
        };
        const actions = [button(item.pinned ? 'isle-pin-fill' : 'isle-pin', item.pinned ? 'Unpin' : 'Pin', () => {
            const pinned = (daemon.state.clipboard ?? []).filter(other => other.pinned).length;
            if (!item.pinned && pinned >= MAX_PINNED) {
                this._flash(item, 'isle-pin', 'Pins full · unpin one');
                return;
            }
            if (!item.pinned)
                this._flash(item, 'isle-pin-fill', 'Pinned');
            daemon.call('Clipboard1', 'SetPinned', {id: item.id, pinned: !item.pinned});
        })];
        if (item.kind === 'text' || item.kind === 'link') {
            actions.push(button('isle-tasks', 'Add to Reminders', () => {
                const title = item.preview.split('\n').find(line => line.trim())?.trim().slice(0, 120);
                if (!title)
                    return;
                daemon.call('todo', 'Add', {title});
                this._flash(item, 'isle-tasks', 'Added to Reminders');
            }));
        } else {
            actions.push(button('isle-shelf', 'Add to the Files tab', async () => {
                const shelf = daemon.state.shelf ?? [];
                const fresh = item.kind === 'image' ? 1 : item.paths.filter(path => !shelf.some(file => file.path === path)).length;
                if (shelf.length + fresh > SHELF_CAPACITY) {
                    this._flash(item, 'isle-shelf', 'Files is full');
                    return;
                }
                const result = await invoke('clipboard_to_shelf', {id: item.id}).catch(() => null);
                this._flash(item, 'isle-shelf', result?.added ? 'Added to Files' : 'Files is full');
            }));
        }
        actions.push(button('isle-close-symbolic', 'Remove', () => {
            // Removed at once; the card shrinks away until the list redraws.
            card.classList.add('removed');
            daemon.call('Clipboard1', 'Remove', {id: item.id});
        }));
        return el('div', 'isle-clip-actions', actions);
    }

    // Clear

    /** Swaps the trash button for "Clear all" for a few seconds, so one click never wipes it. */
    _askToClear() {
        this._clear.hidden = true;
        this._confirm.hidden = false;
        clearTimeout(this._confirmTimer);
        this._confirmTimer = setTimeout(() => this._endConfirm(), CONFIRM_MS);
    }

    _endConfirm() {
        clearTimeout(this._confirmTimer);
        this._confirm.hidden = true;
        this._clear.hidden = false;
    }

    // Page

    /** Opening: the ages move on. Leaving: items copied move to the front, and the search empties. */
    setActive(active) {
        if (active === this._active)
            return;
        this._active = active;
        if (active) {
            this._refreshAges();
            // Files moved or deleted since leave the tray.
            daemon.call('Shelf1', 'Refresh');
            return;
        }
        this._setAdded(false);
        this.dropping = false;
        const items = daemon.state.clipboard ?? [];
        for (const id of this._copied.filter(copied => items.some(item => item.id === copied)))
            daemon.call('Clipboard1', 'Touch', {id});
        this._copied = [];
        if (this._search.value) {
            this._search.value = '';
            this._searchFor('');
        }
        if (document.activeElement === this._search)
            this._search.blur();
        this._closeSearch();
        this._endConfirm();
    }
}
