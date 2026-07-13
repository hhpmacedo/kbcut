# Changelog

## 0.2.0 — unreleased

First public release.

- Layout detection generalized beyond GNOME: Sway, Hyprland, KDE, generic
  X11, and localectl backends; xkb variants supported.
- Live layout tracking — switching layouts no longer needs a daemon restart.
- X11 clipboard fallback (xclip/xsel) for emoji and special characters;
  missing tools now warn at startup instead of failing silently.
- `kbcut setup` actually performs the install (udev rule, input group,
  systemd user unit), idempotently, printing every privileged command first.
- `kbcut doctor` diagnoses the installation with fix hints.
- A malformed config no longer kills the daemon; it starts with zero
  replacements and self-heals when the file is fixed.

## 0.1.0

Initial version: GNOME + Wayland + systemd, manual setup.
