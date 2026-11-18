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
- **Apps** (off until turned on in Settings): time in each app today, over 7 days and over 30, kept only on this
  computer (`%LOCALAPPDATA%\Isle\app-usage.json`) for 30 days; Settings says so, and deletes it.
- **Calls**: a call in WhatsApp, Teams, Zoom, Discord or another call app, or Meet and the like in a browser, found
  by who records from the microphone: a green phone in the closed island (a right click brings the call forward),
  and a Call tab with its length, mute (the microphone's, for every app), a way back, and a live preview of the
  call's window drawn by Windows. Video calls are told apart where Windows still records camera use in the
  registry; insider builds that keep it in a private database show every call as a voice call.
- **Shelf**: the clipboard history, as cards of text, links, images and files (20 recent, 10 pinned), kept in
  `%LOCALAPPDATA%\Isle\clipboard.json` and `clips`. Click a card to copy it; search with ← → ↑ ↓ and Enter to paste
  into the window behind; pin, send text to Reminders, or clear. Copies password managers mark as private, and
  those from programs left out in Settings, are never kept. Its Files tab holds six files: drag files onto the
  island to park them, then drag a tile into any app, click it to copy the file, or double-click to open it. Press
  **Phone** and scan the QR code to get the tray's files on your phone, or send photos and files from it, in the
  phone's browser, on the same Wi-Fi; each phone is let in with Allow. The web server, its sessions and the page
  are isled's own (`crates/isle-daemon/src/share`); it ends with Stop, when the screen locks, the PC sleeps or the
  network changes. Windows may ask to let Isle through its firewall the first time.
- **Alerts**: plugging the charger in or out, a full or low battery (20, 10 and 5%), going offline, and Bluetooth
  devices connecting with their battery, as Windows Settings shows it. Each has a switch in Settings → Island.
- **Dictation** (as LocalFlow does it): hold Right Ctrl, speak, let go, and the words are typed where the cursor
  is; Right Ctrl + Space keeps listening hands-free. The island shows the microphone's wave meanwhile. Speech is
  turned into text on this computer by NVIDIA's Parakeet (English) through sherpa-onnx, linked into isle.exe;
  the model (108 MB) is downloaded once from Settings → Voice, or LocalFlow's own is used. Nothing leaves the PC.
- **Voice assistant**: Win+Alt+I (Super+I on GNOME; Win+I is Windows' Settings), or the Assistant page, to talk
  or type. Claude Code answers with your own sign-in, step by step as on GNOME: Isle looks things up for it
  (read-only PowerShell checks, files, apps), does things on the desktop (apps, windows, sound, Wi-Fi, dark mode,
  reminders, typing at the cursor), and runs a command that changes the computer only after Run. A switch in
  Settings → Voice turns it off. Claude Code only, of the AIs GNOME's assistant offers.
- **Settings**: a click on the tray icon, the gear in the open island, or starting Isle again. Isle starts with
  Windows once installed, hides over full-screen videos and games (not over maximized windows), and warns as an
  AI plan nears its limits.

Hooks talk to Isle over a local port; its number and a random key are in `%LOCALAPPDATA%\Isle\agents.json`.
`ISLE_DEBUG=1` makes the hook say what it does on stderr.

## Still to port

Codex, Grok and the other AIs in the assistant, its bot face, and speech in languages other than English. The
glass look is a
deep smoked tint here: Windows has no blur an always-on-top, never-active window can cut to the island's shape.
`cargo tauri build` makes the installer (`target/release/bundle/nsis`). Isle links the C runtime statically (for
sherpa-onnx); with Build Tools older than 17.12, `src/msvc_stl.rs` fills in the STL helpers its libraries need.
