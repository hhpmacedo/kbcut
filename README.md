# kbcut

Seamless text replacement for Linux, macOS-style. Type `brb` followed by a
space or punctuation and it becomes `be right back` — in every app, including
terminals, on Wayland.

## Install

    cargo install kbcut
    kbcut setup      # udev rule, input group, systemd service — asks before each step
    kbcut doctor     # verify

Log out and back in (group membership applies at login).

Requires Linux with the `uinput` module (default on virtually every distro).
The background service assumes a `systemd --user` session; without one, run
`kbcut daemon` from your session autostart. `kbcut setup --print` shows the
commands without running them.

## How it works

A small daemon reads keyboard events from `/dev/input` (this bypasses
Wayland's keystroke isolation, which is why the setup above is needed) and
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

Prefer `kbcut add` over hand-editing the TOML file directly — it correctly
escapes special characters (backslashes, quotes, punctuation trigger keys).
Hand-editing is how a real bug got hit during development: `\_` inside a
replacement is not a valid TOML escape sequence and fails to parse.

## Layout detection

The active xkb layout is detected automatically on GNOME, KDE, Sway, and
Hyprland, using each desktop's own tooling. On anything else, kbcut falls
back to generic X11 (`setxkbmap`) or, failing that, `localectl`.

Layout switching is now live-tracked: switching keyboard layouts no longer
requires restarting the daemon (this was a limitation in v0.1).

If expansions come out with wrong or garbled characters, the first thing to
try is pinning the layout explicitly in the config, which disables all
detection:

```toml
layout = "pt"
# or with a variant:
layout = "pt(nativo)"
```

## Special characters

Characters not directly typeable on the current layout (emoji, `¯`, `ツ`,
`→`, etc.) are delivered via clipboard paste, using `wl-clipboard` on
Wayland or `xclip`/`xsel` on X11. If none of those are installed, those
characters are skipped and kbcut prints a one-time startup warning naming
which package to install. Plain ASCII-typeable replacements are unaffected
either way — they're typed as real keystrokes, not pasted.

## Known limitations

- **Enter/Tab don't trigger expansion** — they clear the buffer instead.
  In chat apps, Enter would send the unexpanded trigger before the daemon
  could repair it, so only printable boundaries (space, punctuation) expand.
- **Caps Lock**: injected replacement text will come out with inverted case
  while Caps Lock is on.
- **X11's active layout can't be queried directly** — `setxkbmap` doesn't
  expose which of your configured layouts is currently active, so kbcut
  uses the first configured layout there and polls periodically. This is
  best-effort and not planned to be fixed in this release; pin `layout` in
  the config if it picks the wrong one.

## Troubleshooting

Run `kbcut doctor` first — it checks the udev rule, input group membership,
and systemd service, and is also the standard thing to attach to a bug
report. If expansions produce wrong characters, try pinning `layout = "..."`
in the config (see Layout detection above). To watch live logs:

```bash
journalctl --user -u kbcut -f
```
