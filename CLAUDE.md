# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

kbcut is a Linux text-expansion daemon (macOS-style): type a trigger word
followed by a boundary character and it's replaced system-wide, including in
terminals and under Wayland. It works by reading raw keyboard events from
`/dev/input` (bypassing Wayland's per-app keystroke isolation) and injecting
replacement keystrokes through a virtual `uinput` keyboard.

See `README.md` for the full requirements/constraints list (GNOME-only
layout detection, Wayland-only clipboard fallback, udev/`input` group setup)
— that's user-facing documentation and won't be repeated here.

## Commands

```bash
cargo build --release          # optimized binary at target/release/kbcut
cargo test                      # unit tests (currently keymap.rs only)
cargo test <test_name>          # run a single test, e.g. cargo test space_and_letters_are_typeable
cargo test -- --nocapture       # see println! output from tests
```

Manual foreground run (no systemd), useful for iterating without touching
the installed service:

```bash
sudo env HOME=$HOME target/debug/kbcut daemon
```

### Deploying a change to the running service

The binary is locked while the systemd service runs it, and the service
reads real input devices — you can't just rebuild in place:

```bash
systemctl --user stop kbcut
cargo build --release
cp target/release/kbcut ~/.local/bin/
systemctl --user start kbcut
journalctl --user -u kbcut -n 20 --no-pager -o cat   # verify: layout detected, N replacement(s), no "Permission denied"/parse errors
```

## Architecture

Four modules, wired together in `main.rs`:

- **`config.rs`** — `Config { layout: Option<String>, replacements: BTreeMap<String,String> }`, serialized to `~/.config/kbcut/config.toml`. `load()`/`save()` are the only I/O.
- **`keymap.rs`** — wraps `xkbcommon`. Builds a reverse map (`char -> KeyCombo{keycode, shift, altgr}`) by scanning every key/level of a compiled xkb keymap for one layout, so it can answer "which key(s) produce this character?". Also tracks live modifier state (`update()`) to answer `is_modifier_active()` / `is_shortcut_modifier_active()`, and decodes a raw evdev keycode to a `char` given current modifiers (`char_for()`).
- **`inject.rs`** — owns the `uinput` virtual device (`Injector`). `replace(count, text, keymap)` backspaces `count` chars then either types `text` via `keymap.combo_for()` keystrokes, or — if any character isn't reachable on the current layout — falls back to a clipboard paste (`wl-copy`/`wl-paste`), saving and restoring whatever was previously on the clipboard.
- **`daemon.rs`** — the event loop and all the tricky timing logic (see below).

### Event pipeline (daemon.rs)

Everything funnels through one `mpsc::channel<Msg>` consumed by a single loop
in `run()`, fed by several threads:

- One reader thread per keyboard/pointer device under `/dev/input` (`spawn_reader`), matched by capability (`is_keyboard`/`is_pointer`), not by name — device nodes get renumbered across reboots/hotplug.
- `spawn_rescan` polls `/dev/input` every `RESCAN_INTERVAL` to pick up hotplugged devices (Bluetooth reconnects, new USB keyboards).
- `spawn_config_watcher` uses `notify` on the config's parent directory and emits `ReloadConfig` when `config.toml` changes — this is the *only* reload path; there's no signal or IPC-based reload.
- Mouse clicks (`Msg::PointerButton`) reset the in-progress word, since a click likely moved the cursor elsewhere.

`handle_key` is the trigger-matching state machine: it accumulates printable
characters into `word` until a boundary character (`is_boundary` — whitespace
or sentence punctuation; `-_@#` etc. stay part of a word so triggers can use
them), then looks up `word` in `triggers`.

**Why matches don't inject immediately (`Pending`):** the compositor silently
drops an *injected* press of a keycode that's already physically held down.
If you type `brb` + space, the daemon reacts on the space *press*, but your
finger is still holding it — injecting a space right then would vanish. So a
match becomes a `Pending` that only fires (`fire()` → `injector.replace()`)
once the boundary key is physically *released* and no modifier is held. This
also has to handle fast-typing rollover: keys pressed before the boundary key
is released get folded into `Pending.text` (erased and retyped after the
expansion) rather than aborting the match.

Ctrl/Alt/Super chords clear the word buffer (they're commands, not text), and
`is_ours()` filters out kbcut's own virtual device by name so it doesn't
listen to its own injected events.

### Layout detection

`detect_gnome_layout()` shells out to `gsettings` and reads
`org.gnome.desktop.input-sources` `mru-sources` (falling back to `sources`)
to find the layout the user is *currently* typing with — `mru-sources`
reflects actual switch order, `sources` is just the static configured list
and doesn't change when the user switches layouts. This only runs once at
daemon startup (`config.layout` takes precedence if set); switching layouts
while the daemon is running does not update decoding until it's restarted.
