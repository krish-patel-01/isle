# Isle for Windows

The Isle island on Windows 10 and 11: the same black notch at the top of the screen, the same sizes, springs,
fonts, icons and pages as the GNOME extension, in one program instead of a Shell extension and `isled`.

## How it is put together

| GNOME | Windows |
|---|---|
| `gnome-extension/` draws the island inside GNOME Shell | `ui/` draws it in a transparent, always-on-top window (Tauri, WebView2) |
| `isled` serves state over D-Bus (Media1, Pomodoro1, Todo1, …) | `src/` serves the same JSON to the page through Tauri |
| MPRIS for what plays | Windows' media controls (the player in the volume flyout) |
| `isle-core` for the timer, tasks, lyrics parsing | the same `isle-core` |
| `isle-daemon/src/media/lyrics.rs` (LRCLIB) | the same file, compiled in |
| `gnome-extension/icons`, `data/fonts` | copied into `ui/` by `build.rs` at build time |

The window covers the top centre of the screen but lets every click through except on the island itself.

## Build and run

Needs Rust 1.88 or newer, the Visual Studio Build Tools (C++), and WebView2 (part of Windows 11).

```sh
cargo run -p isle-windows            # the island, with a console for its log
cargo build -p isle-windows --release
```

The release build is `target/release/isle.exe`. Quit it from its tray icon. Data lives in `%LOCALAPPDATA%\Isle`.

To work on the island's look without building: `python -m http.server -d crates/isle-windows/ui`, then open
`/preview.html`. It stands in for the Rust side with an invented song and placeholder lines.

## What works

- The closed island: the time, and a song's cover and bars, or the timer's ring and time left, either side.
- Rest the pointer on it to open it (120 ms, as Isle's default); it closes 300 ms after the pointer leaves.
- **Music**: what any player in Windows' media controls plays, with its controls and scrubber; synced lyrics from
  LRCLIB, the island widening for them.
- **Timer**: the Pomodoro timer, its lengths, Start, Pause and Reset; it keeps running after a restart.
- **Reminders**: add, rename, tick off, remove; finished ones clear at midnight.
- A focus session or break that ends chimes (Windows' notification sound) and says so, with Start break / Start focus.
- **Usage**: how much of your Claude and ChatGPT plans' limits is used, read with Claude Code's and Codex's own
  sign-ins (never renewed by Isle), every 10 minutes and when the page opens.
- **Claude Code asks from the island**: turn on *Answer Claude Code from the island* in the tray icon's menu. Isle
  adds its hooks to `~/.claude/settings.json` beside your own (the first version is kept as
  `settings.json.isle-backup`); turning it off takes them out again. A permission shows with **Deny**, **Allow** and,
  when Claude offers to keep a rule, **Always**; answering in Claude's terminal works as before, and nothing shows
  while its window is in front. A finished turn, or Claude waiting for you, shows too.

- **Calendar**: the month, and the day's events from Windows' calendars and Isle's own (add one in a line,
  "Dinner 8:00 PM"); world clocks; an alert before each event with a time and another as it starts, with
  **Join** for a meeting link.
- **Notifications**: other apps' notifications, in the island as they come and listed on their page, with
  Do Not Disturb and Clear. Windows still shows its own banners too.
- **Settings**: a click on the tray icon, the gear in the open island, or starting Isle again. Isle starts with
  Windows once installed, hides over full-screen videos and games (not over maximized windows), and warns as an
  AI plan nears its limits.

Hooks talk to Isle over a local port; its number and a random key are in `%LOCALAPPDATA%\Isle\agents.json`.
`ISLE_DEBUG=1` makes the hook say what it does on stderr.

## Still to port

The clipboard shelf and files tray, phone sharing, calls, the voice assistant and app usage. The glass look is a
deep smoked tint here: Windows has no blur an always-on-top, never-active window can cut to the island's shape.
`cargo tauri build` makes the installer (`target/release/bundle/nsis`).
