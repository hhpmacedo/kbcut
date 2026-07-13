# kbcut v0.2 — Portable Public Release

**Date:** 2026-07-13
**Status:** Approved design, pre-implementation

## Goal

Turn kbcut from a single-machine tool (GNOME + Wayland + systemd, manual
7-step install) into a publicly releasable one. Target: the **modern Linux
baseline** — any desktop environment, Wayland-first with best-effort X11,
systemd assumed for the service (manual run supported without it).

## Scope decisions

- **Audience:** public open-source release (GitHub + crates.io).
- **Environments:** Wayland any-compositor first-class; X11 best-effort
  (never a release blocker); systemd-user for the service; graceful
  degradation everywhere else.
- **Install channel:** `cargo install kbcut && kbcut setup`. No prebuilt
  binaries, curl-installer, or distro packages in this release.
- **Layout:** generalize detection beyond GNOME *and* track live layout
  switches (no more restart-after-switching).

## Architecture

```
src/
  layout/           NEW — backend probing, detection, live watch
    mod.rs            backend selection + watcher thread + keymap swap
    backends.rs       gnome, kde, sway, hyprland, x11, localectl
    registry.rs       descriptive-name -> xkb code (parses evdev.xml)
  clipboard.rs      NEW — paste backend: wl-copy | xclip/xsel | disabled
  setup.rs          NEW — `kbcut setup` and `kbcut doctor` subcommands
  daemon.rs         CHANGED — consume live layout updates via shared keymap
  inject.rs         CHANGED — paste through clipboard.rs abstraction
```

## Layout detection

Backend selection at startup, first match wins:

1. `layout = "..."` in config → **fixed layout, no detection, no watcher**
   (the universal escape hatch; always documented first in troubleshooting)
2. `$SWAYSOCK` → Sway (`swaymsg -t get_inputs`)
3. `$HYPRLAND_INSTANCE_SIGNATURE` → Hyprland (`hyprctl devices -j`)
4. `$XDG_CURRENT_DESKTOP` contains GNOME → gsettings
5. `$XDG_CURRENT_DESKTOP` contains KDE → kxkbrc + qdbus
6. `$DISPLAY` set → generic X11 (`setxkbmap -query`)
7. `localectl status` → system-level layout
8. `"us"` + startup warning naming the config override

Each backend implements:

```rust
trait LayoutBackend {
    fn current(&self) -> Result<(String, Option<String>)>; // (layout, variant)
    fn watch(&self) -> Option<WatchChild>;                  // None => poll
}
```

**Backends return `(layout, variant)`, not a bare layout.** GNOME
`mru-sources` can carry variants (`pt+nativo` style); `Keymap::new`
currently discards them. Variant is threaded through to
`xkb::Keymap::new_from_names`.

**The active-not-first rule (regression-critical).** Every backend must
return the layout *in effect right now*, never the first configured one.
This was development bug #3 (hardcoded/first `us` while `pt` was active →
garbled URL punctuation). Concretely:

- GNOME: `mru-sources` before `sources` (mru's first entry is the active
  layout; `sources` is configured order only) — preserve existing logic.
- Sway: use `xkb_active_layout_index` into `xkb_layout_names`, never `[0]`.
- KDE: D-Bus `org.kde.KeyboardLayouts.getLayout` index into the
  `kxkbrc` `LayoutList`, never the first entry.
- X11: `setxkbmap -query` reports the layout list; active group is not
  exposed there — use first + poll (best-effort per scope, documented).

**Descriptive-name normalization.** Sway and KDE report human names
("Portuguese") rather than xkb codes ("pt"). `registry.rs` parses
`/usr/share/X11/xkb/rules/evdev.xml` (present wherever xkb is) once at
startup into a description → (layout, variant) map. Pure function,
fixture-tested.

## Live layout tracking

Backends that support events expose them uniformly as *spawn a child
process, read stdout lines*:

- GNOME: `gsettings monitor org.gnome.desktop.input-sources mru-sources`
- Sway: `swaymsg -t subscribe -m '["input"]'`
- Hyprland: socket2 event stream
- KDE: `dbus-monitor` on `layoutChanged`
- X11 / localectl: poll `current()` every 3 s

One watcher thread owns the child/poll loop, rebuilds the xkb keymap on
change, and swaps it into an `Arc<RwLock<Keymap>>` read by the daemon per
event — the existing evdev event loop is not restructured.

Rules:

- On keymap swap: clear the `word` buffer **and any `Pending` expansion**
  (never decode one word under two layouts).
- Watcher child dies → fall back to polling.
- Detection call fails → keep last good keymap, log once.

## Special-char paste

`clipboard.rs` selects a backend at startup:

- `$WAYLAND_DISPLAY` → `wl-copy` / `wl-paste`
- else `$DISPLAY` → `xclip`, falling back to `xsel`
- else → disabled

Behavior preserved verbatim from `inject.rs` today: save previous clipboard,
set, Ctrl+V, 150 ms settle delay, restore. Only *which command* is invoked
changes. When disabled or the tool is missing, non-typeable characters are
skipped with a **one-time startup warning naming the package to install** —
replaces today's silent X11 skip.

## `kbcut setup` and `kbcut doctor`

**`setup`** — idempotent, interactive. Prints each privileged command before
running it with sudo:

1. check uinput kernel module
2. install udev rule → reload udev
3. `usermod -aG input $USER`
4. write systemd user unit → `daemon-reload` + `enable`
5. "log out and back in" notice

The udev rule and unit file are embedded via `include_str!` so
`cargo install` needs no repo checkout. `--print` emits the commands without
executing (manual / non-systemd users). No systemd detected → print manual
run instructions instead of failing.

**`doctor`** — pass/fail + fix hint per check; non-zero exit on any failure:

- uinput module loaded; `/dev/uinput` writable
- `input` group membership, including "you haven't re-logged yet" detection
- udev rule installed; input devices readable
- **config parses** (with the TOML error and line on failure)
- layout backend chosen + detected (layout, variant)
- clipboard backend + tool present
- service status

`doctor` doubles as the manual QA harness and the standard bug-report
attachment.

## Preserved behaviors & regression protection

Development bugs and their fixes are load-bearing. The spec freezes them:

| Bug found in development | Fix location | Protection in v0.2 |
|---|---|---|
| TOML invalid escape (`"¯\_(ツ)_/¯"`) crashed parsing | literal strings; `kbcut add` serializes via serde | config-robustness work below + round-trip tests |
| Punctuation trigger keys (`"-->"`) need quoted TOML keys | serde serializer quotes automatically | round-trip tests |
| First-configured layout used instead of active | `mru-sources` before `sources` | active-not-first rule + multi-layout fixtures per backend |
| Injected space vanished while physical space held | `Pending` state machine waits for boundary + modifier release; rollover folding | **frozen** — no changes to this path |
| Non-typeable chars (emoji, `→`, `ツ`) | clipboard paste with save/restore + settle delay | frozen; only the invoked command changes |
| Apps drop too-fast injected events | 2 ms `KEY_DELAY`, 10 ms pre-inject grace | frozen |

**Config robustness (new work).** Today a malformed config at startup kills
the daemon — under systemd, a crash loop, and hand-editing TOML is exactly
how users will hit it. Change: startup parse failure → run with **zero
replacements + loud log**; the existing config watcher already reloads on
change, so the daemon self-heals the moment the file is fixed. README steers
users to `kbcut add` as the primary interface.

## Error-handling philosophy

*Degrade, log once, keep typing.* A text expander must never make the
keyboard feel broken. No path in layout detection, clipboard, or config
handling may take down the daemon after startup.

## Testing

- **Unit (CI):** every backend output parser against captured real-output
  fixtures — each including a multi-layout fixture where the active layout
  is not first; `registry.rs` evdev.xml parsing; backend selection (env-var
  driven, mockable); TOML round-trip of `¯\_(ツ)_/¯`, quoted keys (`"-->"`),
  multiline, emoji; existing keymap tests.
- **Manual QA (uinput/evdev can't run in CI):** per-environment checklist —
  GNOME/Wayland, Sway, X11 fallback — executed via `kbcut doctor` plus a
  scripted trigger test.
- **CI:** GitHub Actions — fmt, clippy, build, unit tests.

## Release hygiene

- `LICENSE` file (MIT, matching Cargo.toml)
- README rewrite: requirements → modern Linux baseline; install =
  `cargo install kbcut && kbcut setup`; per-DE notes; troubleshooting
  (config override first)
- `CHANGELOG.md`
- CI workflow
- Publish `0.2.0` to crates.io

## Non-goals (this release)

- Prebuilt binaries, curl-installer, distro packages (AUR/COPR/PPA)
- Non-systemd service managers (manual run documented instead)
- X11 active-group tracking beyond polling
- Enter/Tab as expansion boundaries (deliberate: chat apps would send the
  unexpanded trigger)
- Caps Lock case inversion fix
- Flatpak/sandboxed distribution (incompatible with /dev/input access)
