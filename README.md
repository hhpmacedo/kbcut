# kbcut

Seamless text replacement for Linux, macOS-style. Type `brb` followed by a
space or punctuation and it becomes `be right back` — in every app, including
terminals, on Wayland.

## Requirements

kbcut is built for a specific machine setup, not a generic cross-desktop tool:

- **Linux only**, with the `uinput` kernel module (default on virtually every
  distro) — this is what lets kbcut inject synthetic keystrokes.
- **systemd user session**, if you want it running in the background. Setup
  installs a `systemd --user` unit; there's no other supported service
  manager. You can still run `kbcut daemon` by hand without systemd.
- **GNOME**, for automatic keyboard layout detection. kbcut shells out to
  `gsettings get org.gnome.desktop.input-sources mru-sources` to find the
  layout you're *currently* typing with. On any other desktop this call
  fails silently and kbcut falls back to `us` — set `layout` explicitly in
  the config if that's wrong for you.
- **Wayland, for replacements with characters outside your keyboard layout**
  (emoji, `¯`, `ツ`, etc.). Those are delivered via clipboard paste using
  `wl-copy`/`wl-paste` (`wl-clipboard` package), which is Wayland-only.
  Under X11 those characters are silently skipped — there's no X11 paste
  fallback implemented. Plain-ASCII replacements are unaffected either way,
  since they're typed as real keystrokes, not pasted.
- Your user must be in the `input` group and have the udev rule below
  installed, granting read access to `/dev/input/*` and write access to
  `/dev/uinput`.

## How it works

A small daemon reads keyboard events from `/dev/input` (this bypasses
Wayland's keystroke isolation, which is why the setup below is needed) and
decodes them with your xkb layout. When a trigger word is followed by a word
boundary (space, `.,;:!?`, brackets, quotes), it injects backspaces and the
replacement text through a virtual `uinput` keyboard. Because the daemon can
see all keystrokes, it runs as *your* user, locally, and never writes
anything to disk.

## Usage

```bash
kbcut add brb "be right back"   # add or update
kbcut rm brb                    # remove
kbcut list                      # show all
```

Replacements live in `~/.config/kbcut/config.toml`; the daemon reloads it
automatically when it changes. Multi-line replacements work (`\n` in TOML).

## One-time setup

```bash
cargo build --release

# Allow your user to read input devices and use uinput
sudo cp packaging/99-kbcut-uinput.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo usermod -aG input $USER

# Install binary + autostart service
mkdir -p ~/.local/bin ~/.config/systemd/user
cp target/release/kbcut ~/.local/bin/
cp packaging/kbcut.service ~/.config/systemd/user/
systemctl --user daemon-reload && systemctl --user enable kbcut
```

Log out and back in (group membership applies at login), then:

```bash
systemctl --user start kbcut
journalctl --user -u kbcut -f    # watch logs
```

## Configuration

`~/.config/kbcut/config.toml`:

```toml
# Optional: override the xkb layout used to decode keys.
# Defaults to your first GNOME input source.
# layout = "pt"

[replacements]
brb = "be right back"
omw = "on my way!"
```

## Known limitations (v1)

- **Layout is detected once at startup**: kbcut picks up whichever xkb
  layout GNOME reports as currently active (or `layout` from the config)
  when the daemon starts, then keeps using it. If you switch layouts
  (us ⇄ pt) while the daemon is running, decoding keeps following the
  layout from startup until you restart it.
- **Enter/Tab don't trigger expansion** — they clear the buffer instead.
  In chat apps, Enter would send the unexpanded trigger before the daemon
  could repair it, so only printable boundaries (space, punctuation) expand.
- **Caps Lock**: injected replacement text will come out with inverted case
  while Caps Lock is on.
- Characters not reachable on your layout (emoji, `¯`, `ツ`, etc.) are
  pasted via the clipboard on Wayland; on X11 they're silently skipped
  (see Requirements above).
