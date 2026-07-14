//! Per-environment detection of the ACTIVE xkb layout.
//!
//! REGRESSION-CRITICAL: every backend returns the layout in effect right
//! now, never the first configured one. (Dev bug: first-configured "us"
//! used while "pt" was active garbled all punctuation.)

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::io::BufReader;
use std::process::Command;
use std::process::Stdio;

use super::registry::Registry;
use super::LayoutSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Gnome,
    Kde,
    Sway,
    Hyprland,
    X11,
    Localectl,
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Gnome => "gnome",
            Backend::Kde => "kde",
            Backend::Sway => "sway",
            Backend::Hyprland => "hyprland",
            Backend::X11 => "x11",
            Backend::Localectl => "localectl",
        }
    }
}

/// Pick a backend from the environment. Order matters (spec §Layout
/// detection): compositor sockets are more specific than XDG_CURRENT_DESKTOP,
/// which is more specific than DISPLAY.
pub fn select_backend(get: impl Fn(&str) -> Option<String>) -> Option<Backend> {
    if get("SWAYSOCK").is_some() {
        return Some(Backend::Sway);
    }
    if get("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        return Some(Backend::Hyprland);
    }
    if let Some(desktop) = get("XDG_CURRENT_DESKTOP") {
        let d = desktop.to_uppercase();
        if d.contains("GNOME") {
            return Some(Backend::Gnome);
        }
        if d.contains("KDE") {
            return Some(Backend::Kde);
        }
    }
    if get("DISPLAY").is_some() {
        return Some(Backend::X11);
    }
    Some(Backend::Localectl)
}

impl Backend {
    /// The layout in effect right now.
    pub fn current(&self, registry: &Registry) -> Result<LayoutSpec> {
        match self {
            Backend::Gnome => {
                // mru-sources tracks switch order; its first entry is active.
                // `sources` is configured order only — fallback, not primary.
                for key in ["mru-sources", "sources"] {
                    let out = run(
                        "gsettings",
                        &["get", "org.gnome.desktop.input-sources", key],
                    )?;
                    if let Some(spec) = parse_gnome_sources(&out) {
                        return Ok(spec);
                    }
                }
                Err(anyhow!("no xkb entry in gsettings input-sources"))
            }
            Backend::Kde => {
                let layouts = kde_read("LayoutList")?;
                let variants = kde_read("VariantList").unwrap_or_default();
                let index = kde_active_index().unwrap_or(0);
                parse_kde(&layouts, &variants, index)
                    .ok_or_else(|| anyhow!("kxkbrc LayoutList empty or index out of range"))
            }
            Backend::Sway => {
                let out = run("swaymsg", &["-t", "get_inputs", "--raw"])?;
                parse_sway_inputs(&out, registry)
                    .ok_or_else(|| anyhow!("no keyboard with an active layout in swaymsg output"))
            }
            Backend::Hyprland => {
                let out = run("hyprctl", &["devices", "-j"])?;
                parse_hyprctl_devices(&out, registry)
                    .ok_or_else(|| anyhow!("no keyboard in hyprctl devices output"))
            }
            Backend::X11 => {
                // setxkbmap doesn't expose the active group — first entry,
                // best-effort per spec (documented X11 limitation).
                let out = run("setxkbmap", &["-query"])?;
                parse_setxkbmap(&out).ok_or_else(|| anyhow!("no layout in setxkbmap -query"))
            }
            Backend::Localectl => {
                let out = run("localectl", &["status"])?;
                parse_localectl(&out).ok_or_else(|| anyhow!("no X11 Layout in localectl status"))
            }
        }
    }
}

fn run(bin: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("running {bin}"))?;
    if !out.status.success() {
        return Err(anyhow!("{bin} exited with {}", out.status));
    }
    String::from_utf8(out.stdout).with_context(|| format!("{bin} output not UTF-8"))
}

fn kde_read(key: &str) -> Result<String> {
    for bin in ["kreadconfig6", "kreadconfig5"] {
        if let Ok(v) = run(
            bin,
            &["--file", "kxkbrc", "--group", "Layout", "--key", key],
        ) {
            return Ok(v.trim().to_string());
        }
    }
    Err(anyhow!("kreadconfig not available"))
}

fn kde_active_index() -> Result<usize> {
    for bin in ["qdbus6", "qdbus"] {
        if let Ok(v) = run(
            bin,
            &[
                "org.kde.keyboard",
                "/Layouts",
                "org.kde.KeyboardLayouts.getLayout",
            ],
        ) {
            return v
                .trim()
                .parse::<usize>()
                .context("parsing KDE layout index");
        }
    }
    Err(anyhow!("qdbus not available"))
}

// ── pure parsers (fixture-tested) ──────────────────────────────────────────

/// `[('xkb', 'pt+nativo'), ('xkb', 'us')]` → pt(nativo). First entry = active.
pub fn parse_gnome_sources(text: &str) -> Option<LayoutSpec> {
    let start = text.find("('xkb', '")? + "('xkb', '".len();
    let rest = &text[start..];
    let end = rest.find('\'')?;
    let value = &rest[..end];
    Some(match value.split_once('+') {
        Some((l, v)) => LayoutSpec::new(l, Some(v)),
        None => LayoutSpec::new(value, None::<String>),
    })
}

/// LayoutList="us,pt" VariantList=",nativo" index=1 → pt(nativo).
/// NEVER just the first entry — index selects the active layout.
pub fn parse_kde(layout_list: &str, variant_list: &str, index: usize) -> Option<LayoutSpec> {
    let layouts: Vec<&str> = layout_list.split(',').map(str::trim).collect();
    let variants: Vec<&str> = variant_list.split(',').map(str::trim).collect();
    let layout = layouts.get(index).filter(|l| !l.is_empty())?;
    let variant = variants.get(index).filter(|v| !v.is_empty());
    Some(LayoutSpec::new(*layout, variant.copied()))
}

#[derive(Deserialize)]
struct SwayInput {
    #[serde(rename = "type")]
    kind: String,
    xkb_active_layout_name: Option<String>,
}

/// Uses xkb_active_layout_name (descriptive → registry). NEVER
/// xkb_layout_names[0] — that's the configured order, not the active layout.
pub fn parse_sway_inputs(json: &str, registry: &Registry) -> Option<LayoutSpec> {
    let inputs: Vec<SwayInput> = serde_json::from_str(json).ok()?;
    inputs
        .iter()
        .filter(|i| i.kind == "keyboard")
        .filter_map(|i| i.xkb_active_layout_name.as_deref())
        .find_map(|name| registry.resolve(name))
}

#[derive(Deserialize)]
struct HyprDevices {
    keyboards: Vec<HyprKeyboard>,
}

#[derive(Deserialize)]
struct HyprKeyboard {
    active_keymap: String,
    #[serde(default)]
    main: bool,
}

pub fn parse_hyprctl_devices(json: &str, registry: &Registry) -> Option<LayoutSpec> {
    let devices: HyprDevices = serde_json::from_str(json).ok()?;
    let kb = devices
        .keyboards
        .iter()
        .find(|k| k.main)
        .or_else(|| devices.keyboards.first())?;
    registry.resolve(&kb.active_keymap)
}

/// `layout:     us,pt` / `variant:    ,nativo` → first entry (best-effort).
pub fn parse_setxkbmap(text: &str) -> Option<LayoutSpec> {
    let field = |name: &str| -> Option<String> {
        text.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string())
    };
    let layouts = field("layout")?;
    let layout = layouts.split(',').next()?.trim().to_string();
    if layout.is_empty() {
        return None;
    }
    let variant = field("variant")
        .and_then(|v| v.split(',').next().map(|s| s.trim().to_string()))
        .filter(|v| !v.is_empty());
    Some(LayoutSpec { layout, variant })
}

/// `    X11 Layout: pt` → pt.
pub fn parse_localectl(text: &str) -> Option<LayoutSpec> {
    let value = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("X11 Layout:"))?
        .trim();
    if value.is_empty() || value == "(unset)" || value == "n/a" {
        return None;
    }
    let layout = value.split(',').next()?.trim();
    Some(LayoutSpec::new(layout, None::<String>))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::from_xml(
            r#"<layoutList>
  <layout>
    <configItem>
      <name>us</name>
      <description>English (US)</description>
    </configItem>
  </layout>
  <layout>
    <configItem>
      <name>pt</name>
      <description>Portuguese</description>
    </configItem>
    <variantList>
      <variant>
        <configItem>
          <name>nativo</name>
          <description>Portuguese (Nativo)</description>
        </configItem>
      </variant>
    </variantList>
  </layout>
</layoutList>"#,
        )
    }

    #[test]
    fn select_backend_priority() {
        let env = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, String)> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            move |key: &str| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        };
        assert_eq!(
            select_backend(env(&[
                ("SWAYSOCK", "/run/sway.sock"),
                ("XDG_CURRENT_DESKTOP", "GNOME")
            ])),
            Some(Backend::Sway)
        );
        assert_eq!(
            select_backend(env(&[
                ("HYPRLAND_INSTANCE_SIGNATURE", "abc"),
                ("DISPLAY", ":0")
            ])),
            Some(Backend::Hyprland)
        );
        assert_eq!(
            select_backend(env(&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")])),
            Some(Backend::Gnome)
        );
        assert_eq!(
            select_backend(env(&[("XDG_CURRENT_DESKTOP", "KDE"), ("DISPLAY", ":0")])),
            Some(Backend::Kde)
        );
        assert_eq!(
            select_backend(env(&[("DISPLAY", ":0")])),
            Some(Backend::X11)
        );
        assert_eq!(select_backend(env(&[])), Some(Backend::Localectl));
    }

    // ACTIVE-NOT-FIRST fixtures: active layout is never first in these.

    #[test]
    fn gnome_mru_first_entry_is_active() {
        // pt active, us configured first historically — mru puts pt first.
        let spec = parse_gnome_sources("[('xkb', 'pt'), ('xkb', 'us')]").unwrap();
        assert_eq!(spec, LayoutSpec::new("pt", None::<String>));
    }

    #[test]
    fn gnome_variant_plus_syntax() {
        let spec = parse_gnome_sources("[('xkb', 'pt+nativo'), ('xkb', 'us')]").unwrap();
        assert_eq!(spec, LayoutSpec::new("pt", Some("nativo")));
    }

    #[test]
    fn kde_index_selects_active_not_first() {
        let spec = parse_kde("us,pt", ",nativo", 1).unwrap();
        assert_eq!(spec, LayoutSpec::new("pt", Some("nativo")));
        let spec = parse_kde("us,pt", "", 0).unwrap();
        assert_eq!(spec, LayoutSpec::new("us", None::<String>));
    }

    #[test]
    fn sway_active_layout_not_first() {
        let json = r#"[
          {"identifier":"1:1:AT_Keyboard","type":"keyboard",
           "xkb_layout_names":["English (US)","Portuguese"],
           "xkb_active_layout_index":1,
           "xkb_active_layout_name":"Portuguese"},
          {"identifier":"2:7:Mouse","type":"pointer"}
        ]"#;
        let spec = parse_sway_inputs(json, &registry()).unwrap();
        assert_eq!(spec, LayoutSpec::new("pt", None::<String>));
    }

    #[test]
    fn hyprland_main_keyboard_descriptive_name() {
        let json = r#"{"mice":[],"keyboards":[
          {"name":"at-keyboard","active_keymap":"English (US)","main":false},
          {"name":"usb-kb","active_keymap":"Portuguese (Nativo)","main":true}
        ]}"#;
        let spec = parse_hyprctl_devices(json, &registry()).unwrap();
        assert_eq!(spec, LayoutSpec::new("pt", Some("nativo")));
    }

    #[test]
    fn setxkbmap_first_entry_with_variant_alignment() {
        let out = "rules:      evdev\nmodel:      pc105\nlayout:     pt,us\nvariant:    nativo,\n";
        let spec = parse_setxkbmap(out).unwrap();
        assert_eq!(spec, LayoutSpec::new("pt", Some("nativo")));
        let out = "rules:      evdev\nlayout:     us\n";
        assert_eq!(
            parse_setxkbmap(out).unwrap(),
            LayoutSpec::new("us", None::<String>)
        );
    }

    #[test]
    fn localectl_x11_layout_line() {
        let out = "   System Locale: LANG=en_US.UTF-8\n       VC Keymap: pt-latin1\n      X11 Layout: pt\n       X11 Model: pc105\n";
        assert_eq!(
            parse_localectl(out).unwrap(),
            LayoutSpec::new("pt", None::<String>)
        );
        assert!(parse_localectl("X11 Layout: (unset)\n").is_none());
    }
}

/// A line stream whose every line means "the layout may have changed —
/// re-detect". Child stdout for CLI monitors, a unix socket for Hyprland.
pub type WatchStream = Box<dyn std::io::BufRead + Send>;

impl Backend {
    /// None → caller polls `current()` instead (X11, localectl).
    pub fn watch_stream(&self) -> Option<WatchStream> {
        match self {
            Backend::Gnome => spawn_lines(
                "gsettings",
                &["monitor", "org.gnome.desktop.input-sources", "mru-sources"],
            ),
            Backend::Sway => spawn_lines("swaymsg", &["-t", "subscribe", "-m", r#"["input"]"#]),
            Backend::Kde => spawn_lines(
                "dbus-monitor",
                &["type='signal',interface='org.kde.KeyboardLayouts',member='layoutChanged'"],
            ),
            Backend::Hyprland => {
                let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
                let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
                let path = format!("{runtime}/hypr/{sig}/.socket2.sock");
                let stream = std::os::unix::net::UnixStream::connect(path).ok()?;
                Some(Box::new(BufReader::new(stream)))
            }
            Backend::X11 | Backend::Localectl => None,
        }
    }
}

fn spawn_lines(bin: &str, args: &[&str]) -> Option<WatchStream> {
    let child = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    Some(Box::new(BufReader::new(child.stdout?)))
}
