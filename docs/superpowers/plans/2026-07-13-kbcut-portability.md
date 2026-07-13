# kbcut v0.2 Portable Public Release — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generalize kbcut beyond GNOME+Wayland+systemd (layout backends with live tracking, X11 clipboard fallback, `kbcut setup`/`doctor`) and ship 0.2.0 to crates.io.

**Architecture:** A `layout/` module detects the active xkb layout via per-environment backends (subprocess/socket line streams for live tracking, poll fallback), sending `Msg::LayoutChanged` into the daemon's existing mpsc loop, which rebuilds the keymap and clears in-flight state. A `clipboard.rs` abstraction replaces hardcoded wl-copy. A `setup.rs` implements real `setup` and `doctor` subcommands with embedded assets.

**Tech Stack:** Rust 2021; existing deps (evdev 0.12, xkbcommon 0.7, clap 4, serde, toml, notify, anyhow, dirs) plus `serde_json = "1"`.

**Spec:** `docs/superpowers/specs/2026-07-13-kbcut-portability-design.md` — read it first; its "Preserved behaviors" table lists code paths that MUST NOT change behavior (the `Pending` state machine in daemon.rs, clipboard save/restore + 150ms settle, 2ms key pacing).

---

### Task 1: Keymap variant support

**Files:**
- Modify: `src/keymap.rs:20-35` (signature), `src/daemon.rs:47` (call site)

- [ ] **Step 1: Baseline check**

Run: `cargo test`
Expected: PASS (1 test: `space_and_letters_are_typeable`)

- [ ] **Step 2: Add failing test to `src/keymap.rs` tests module**

```rust
    #[test]
    fn variant_compiles_and_changes_map() {
        // "us" vs "us(intl)": intl has dead keys; plain "us" must still work
        // through the two-arg signature.
        let plain = Keymap::new("us", "").unwrap();
        assert!(plain.combo_for('a').is_some());
        let intl = Keymap::new("us", "intl").unwrap();
        assert!(intl.combo_for('a').is_some());
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test variant_compiles`
Expected: FAIL — `Keymap::new` takes 1 argument

- [ ] **Step 4: Change the signature**

In `src/keymap.rs`, change `Keymap::new`:

```rust
    pub fn new(layout: &str, variant: &str) -> Result<Self> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",      // rules
            "",      // model
            layout,  // layout
            variant, // variant
            None,    // options
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| {
            anyhow!("failed to compile xkb keymap for layout '{layout}' variant '{variant}'")
        })?;
        let state = xkb::State::new(&keymap);
        let reverse = build_reverse_map(&keymap);
        Ok(Self { state, reverse })
    }
```

Update the existing test's constructor to `Keymap::new("us", "")` and the call in `src/daemon.rs:47` to `Keymap::new(&layout, "")` (temporary — Task 6 replaces it).

- [ ] **Step 5: Run tests, verify all pass**

Run: `cargo test`
Expected: PASS (2 tests)

- [ ] **Step 6: Commit**

```bash
git add src/keymap.rs src/daemon.rs
git commit -m "keymap: accept xkb variant"
```

---

### Task 2: Config robustness + TOML regression tests

**Files:**
- Modify: `src/config.rs` (tests), `src/daemon.rs:39` (soft-load)

- [ ] **Step 1: Add regression tests to `src/config.rs`**

These encode development bugs #1 and #2 (invalid TOML escapes, punctuation trigger keys). Append:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(cfg: &Config) -> Config {
        let raw = toml::to_string_pretty(cfg).unwrap();
        toml::from_str(&raw).unwrap()
    }

    #[test]
    fn roundtrip_backslashes_quotes_emoji_multiline() {
        let mut cfg = Config::default();
        cfg.replacements
            .insert("zshrug".into(), r"¯\_(ツ)_/¯".into());
        cfg.replacements
            .insert("zquote".into(), r#"she said "hi""#.into());
        cfg.replacements.insert("zparty".into(), "🎉🎉".into());
        cfg.replacements
            .insert("zsig".into(), "Hugo\nhumanready.io\n".into());
        assert_eq!(roundtrip(&cfg).replacements, cfg.replacements);
    }

    #[test]
    fn roundtrip_punctuation_trigger_keys() {
        // Bare TOML keys can't contain '>' — serializer must quote them.
        let mut cfg = Config::default();
        cfg.replacements.insert("-->".into(), "→".into());
        cfg.replacements.insert("(c)".into(), "©".into());
        assert_eq!(roundtrip(&cfg).replacements, cfg.replacements);
    }

    #[test]
    fn invalid_escape_is_a_parse_error_not_a_panic() {
        // The literal development bug: \_ is not a valid TOML escape.
        let raw = "[replacements]\nzshrug = \"¯\\_(ツ)_/¯\"\n";
        assert!(toml::from_str::<Config>(raw).is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib config`
Expected: PASS (3 new tests)

- [ ] **Step 3: Daemon must survive a malformed config at startup**

In `src/daemon.rs`, replace `let cfg = config::load()?;` (line 39) with:

```rust
    let cfg = config::load().unwrap_or_else(|e| {
        eprintln!(
            "kbcut: config is invalid, starting with no replacements \
             (fix the file and it reloads automatically): {e:#}"
        );
        config::Config::default()
    });
```

Note: `kbcut add`/`rm`/`list` in main.rs keep the hard error — never rewrite a file we couldn't parse.

- [ ] **Step 4: Build + run tests**

Run: `cargo test`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/daemon.rs
git commit -m "config: TOML regression tests; daemon survives malformed config"
```

---

### Task 3: Layout registry (descriptive name → xkb code)

**Files:**
- Create: `src/layout/mod.rs`, `src/layout/registry.rs`
- Modify: `src/main.rs:1-4` (module decl)

- [ ] **Step 1: Create module skeleton**

`src/layout/mod.rs`:

```rust
pub mod registry;

/// An xkb layout selection: layout code plus optional variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutSpec {
    pub layout: String,
    pub variant: Option<String>,
}

impl LayoutSpec {
    pub fn new(layout: impl Into<String>, variant: Option<impl Into<String>>) -> Self {
        Self { layout: layout.into(), variant: variant.map(Into::into) }
    }

    /// Parse a config-file value: "pt" or "pt(nativo)".
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if let Some((layout, rest)) = s.split_once('(') {
            let variant = rest.trim_end_matches(')').trim();
            if !variant.is_empty() {
                return Self::new(layout.trim(), Some(variant));
            }
        }
        Self::new(s, None::<String>)
    }
}

impl std::fmt::Display for LayoutSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.variant {
            Some(v) => write!(f, "{}({})", self.layout, v),
            None => write!(f, "{}", self.layout),
        }
    }
}
```

In `src/main.rs` add `mod layout;` after `mod keymap;`.

- [ ] **Step 2: Write failing registry tests**

`src/layout/registry.rs`:

```rust
//! Maps descriptive layout names ("Portuguese (Nativo)") to xkb codes
//! ("pt", Some("nativo")) by parsing the xkb registry, evdev.xml.
//! Needed because Sway and KDE report descriptions, not codes.

use std::collections::HashMap;

use super::LayoutSpec;

const EVDEV_XML_PATHS: &[&str] = &[
    "/usr/share/X11/xkb/rules/evdev.xml",
    "/usr/share/xkeyboard-config-2/rules/evdev.xml", // some distros
];

pub struct Registry {
    by_description: HashMap<String, LayoutSpec>,
}

impl Registry {
    pub fn load() -> Self {
        for path in EVDEV_XML_PATHS {
            if let Ok(xml) = std::fs::read_to_string(path) {
                return Self::from_xml(&xml);
            }
        }
        eprintln!("kbcut: xkb registry (evdev.xml) not found; descriptive layout names won't resolve");
        Self { by_description: HashMap::new() }
    }

    pub fn from_xml(xml: &str) -> Self {
        Self { by_description: parse_evdev_xml(xml) }
    }

    /// Resolve either an xkb code ("pt") or a description ("Portuguese").
    pub fn resolve(&self, name: &str) -> Option<LayoutSpec> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        if let Some(spec) = self.by_description.get(name) {
            return Some(spec.clone());
        }
        // Already a code (short, ascii-lowercase, possibly "pt+nativo")
        if name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '_') {
            return Some(match name.split_once('+') {
                Some((l, v)) => LayoutSpec::new(l, Some(v)),
                None => LayoutSpec::new(name, None::<String>),
            });
        }
        None
    }
}

/// Line-oriented parse of evdev.xml. Only the <layoutList> section matters;
/// <modelList>/<optionList> also contain <name>/<description> pairs and must
/// be ignored. Structure inside layoutList:
///   <layout><configItem><name>pt</name>…<description>Portuguese</description>…
///     <variantList><variant><configItem><name>nativo</name>
///       <description>Portuguese (Nativo)</description>…
fn parse_evdev_xml(xml: &str) -> HashMap<String, LayoutSpec> {
    let mut map = HashMap::new();
    let mut in_layout_list = false;
    let mut in_variant = false;
    let mut current_layout: Option<String> = None;
    let mut pending_name: Option<String> = None;

    for line in xml.lines() {
        let line = line.trim();
        if line.starts_with("<layoutList") {
            in_layout_list = true;
        } else if line.starts_with("</layoutList") {
            break;
        }
        if !in_layout_list {
            continue;
        }
        if line.starts_with("<layout>") {
            in_variant = false;
            pending_name = None;
        } else if line.starts_with("<variant") {
            in_variant = true;
            pending_name = None;
        } else if let Some(name) = tag_content(line, "name") {
            if pending_name.is_none() {
                pending_name = Some(name.to_string());
            }
        } else if let Some(desc) = tag_content(line, "description") {
            if let Some(name) = pending_name.take() {
                if in_variant {
                    if let Some(layout) = &current_layout {
                        map.insert(desc.to_string(), LayoutSpec::new(layout.clone(), Some(name)));
                    }
                } else {
                    current_layout = Some(name.clone());
                    map.insert(desc.to_string(), LayoutSpec::new(name, None::<String>));
                }
            }
        }
    }
    map
}

fn tag_content<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = line.find(&open)? + open.len();
    let end = line.find(&close)?;
    (start <= end).then(|| &line[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
<xkbConfigRegistry>
  <modelList>
    <model><configItem><name>pc105</name><description>Generic 105-key PC</description></configItem></model>
  </modelList>
  <layoutList>
    <layout>
      <configItem>
        <name>us</name>
        <shortDescription>en</shortDescription>
        <description>English (US)</description>
      </configItem>
      <variantList>
        <variant>
          <configItem>
            <name>intl</name>
            <description>English (US, intl., with dead keys)</description>
          </configItem>
        </variant>
      </variantList>
    </layout>
    <layout>
      <configItem>
        <name>pt</name>
        <shortDescription>pt</shortDescription>
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
  </layoutList>
  <optionList>
    <group><configItem><name>grp</name><description>Switching to another layout</description></configItem></group>
  </optionList>
</xkbConfigRegistry>
"#;

    #[test]
    fn resolves_layout_descriptions() {
        let r = Registry::from_xml(FIXTURE);
        assert_eq!(r.resolve("Portuguese"), Some(LayoutSpec::new("pt", None::<String>)));
        assert_eq!(r.resolve("English (US)"), Some(LayoutSpec::new("us", None::<String>)));
    }

    #[test]
    fn resolves_variant_descriptions() {
        let r = Registry::from_xml(FIXTURE);
        assert_eq!(r.resolve("Portuguese (Nativo)"), Some(LayoutSpec::new("pt", Some("nativo"))));
    }

    #[test]
    fn passes_codes_through_and_splits_plus() {
        let r = Registry::from_xml(FIXTURE);
        assert_eq!(r.resolve("pt"), Some(LayoutSpec::new("pt", None::<String>)));
        assert_eq!(r.resolve("pt+nativo"), Some(LayoutSpec::new("pt", Some("nativo"))));
    }

    #[test]
    fn ignores_models_and_options() {
        let r = Registry::from_xml(FIXTURE);
        assert_eq!(r.resolve("Generic 105-key PC"), None);
        assert_eq!(r.resolve("Switching to another layout"), None);
    }

    #[test]
    fn real_system_registry_parses_if_present() {
        // Smoke test against the actual file where available (dev machines, CI).
        if std::path::Path::new("/usr/share/X11/xkb/rules/evdev.xml").exists() {
            let r = Registry::load();
            assert_eq!(r.resolve("Portuguese"), Some(LayoutSpec::new("pt", None::<String>)));
        }
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib layout::registry`
Expected: PASS (5 tests)

- [ ] **Step 4: Also test LayoutSpec::parse** — append to `src/layout/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_layout_values() {
        assert_eq!(LayoutSpec::parse("pt"), LayoutSpec::new("pt", None::<String>));
        assert_eq!(LayoutSpec::parse("pt(nativo)"), LayoutSpec::new("pt", Some("nativo")));
        assert_eq!(LayoutSpec::parse(" us "), LayoutSpec::new("us", None::<String>));
    }
}
```

Run: `cargo test --lib layout`
Expected: PASS (6 tests)

- [ ] **Step 5: Commit**

```bash
git add src/layout/ src/main.rs
git commit -m "layout: registry mapping descriptive names to xkb codes"
```

---

### Task 4: Layout backends — detection parsers

**Files:**
- Create: `src/layout/backends.rs`
- Modify: `src/layout/mod.rs` (add `pub mod backends;`), `Cargo.toml` (serde_json)

Every parser is a pure function over captured fixture strings. **Regression-critical rule (dev bug #3): return the ACTIVE layout, never the first configured.** Every multi-layout fixture below has the active layout NOT first.

- [ ] **Step 1: Add serde_json**

In `Cargo.toml` under `[dependencies]`: `serde_json = "1"`

- [ ] **Step 2: Write `src/layout/backends.rs`** (parsers + tests; process plumbing comes in Step 4)

```rust
//! Per-environment detection of the ACTIVE xkb layout.
//!
//! REGRESSION-CRITICAL: every backend returns the layout in effect right
//! now, never the first configured one. (Dev bug: first-configured "us"
//! used while "pt" was active garbled all punctuation.)

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::process::Command;

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
        if let Ok(v) = run(bin, &["--file", "kxkbrc", "--group", "Layout", "--key", key]) {
            return Ok(v.trim().to_string());
        }
    }
    Err(anyhow!("kreadconfig not available"))
}

fn kde_active_index() -> Result<usize> {
    for bin in ["qdbus6", "qdbus"] {
        if let Ok(v) = run(bin, &["org.kde.keyboard", "/Layouts", "org.kde.KeyboardLayouts.getLayout"]) {
            return v.trim().parse::<usize>().context("parsing KDE layout index");
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
  <layout><configItem><name>us</name><description>English (US)</description></configItem></layout>
  <layout><configItem><name>pt</name><description>Portuguese</description></configItem>
    <variantList><variant><configItem><name>nativo</name><description>Portuguese (Nativo)</description></configItem></variant></variantList>
  </layout>
</layoutList>"#,
        )
    }

    #[test]
    fn select_backend_priority() {
        let env = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, String)> =
                vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            move |key: &str| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        };
        assert_eq!(
            select_backend(env(&[("SWAYSOCK", "/run/sway.sock"), ("XDG_CURRENT_DESKTOP", "GNOME")])),
            Some(Backend::Sway)
        );
        assert_eq!(
            select_backend(env(&[("HYPRLAND_INSTANCE_SIGNATURE", "abc"), ("DISPLAY", ":0")])),
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
        assert_eq!(select_backend(env(&[("DISPLAY", ":0")])), Some(Backend::X11));
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
        assert_eq!(parse_setxkbmap(out).unwrap(), LayoutSpec::new("us", None::<String>));
    }

    #[test]
    fn localectl_x11_layout_line() {
        let out = "   System Locale: LANG=en_US.UTF-8\n       VC Keymap: pt-latin1\n      X11 Layout: pt\n       X11 Model: pc105\n";
        assert_eq!(parse_localectl(out).unwrap(), LayoutSpec::new("pt", None::<String>));
        assert!(parse_localectl("X11 Layout: (unset)\n").is_none());
    }
}
```

Add `pub mod backends;` to `src/layout/mod.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo test --lib layout::backends`
Expected: PASS (8 tests)

- [ ] **Step 4: Commit**

```bash
git add src/layout/backends.rs src/layout/mod.rs Cargo.toml Cargo.lock
git commit -m "layout: backends for gnome/kde/sway/hyprland/x11/localectl (active-not-first fixtures)"
```

---

### Task 5: Live watch streams + watcher thread

**Files:**
- Modify: `src/layout/backends.rs` (watch streams), `src/layout/mod.rs` (init + watcher)

- [ ] **Step 1: Add watch streams to `src/layout/backends.rs`**

Append (uses `std::io::BufRead`; add `use std::io::BufReader;` and `use std::process::Stdio;` to the imports):

```rust
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
```

- [ ] **Step 2: Add init + watcher to `src/layout/mod.rs`**

Append:

```rust
use std::sync::Arc;
use std::time::Duration;

use backends::Backend;
use registry::Registry;

const POLL_INTERVAL: Duration = Duration::from_secs(3);

pub struct Detection {
    pub spec: LayoutSpec,
    /// None when the config pinned the layout — no watcher then.
    pub backend: Option<Backend>,
    pub registry: Arc<Registry>,
}

/// Resolve the layout to start with. Config override wins and disables
/// detection entirely (spec: the universal escape hatch).
pub fn init(config_layout: Option<&str>) -> Detection {
    if let Some(value) = config_layout {
        let spec = LayoutSpec::parse(value);
        eprintln!("kbcut: layout '{spec}' pinned by config, detection disabled");
        return Detection { spec, backend: None, registry: Arc::new(Registry::from_xml("")) };
    }
    let registry = Arc::new(Registry::load());
    let backend = backends::select_backend(|k| std::env::var(k).ok());
    let (spec, backend) = match backend {
        Some(b) => match b.current(&registry) {
            Ok(spec) => {
                eprintln!("kbcut: layout '{spec}' via {} backend", b.name());
                (spec, Some(b))
            }
            Err(e) => {
                eprintln!(
                    "kbcut: {} layout detection failed ({e:#}); using 'us'. \
                     Set `layout = \"...\"` in the config to override.",
                    b.name()
                );
                (LayoutSpec::new("us", None::<String>), Some(b))
            }
        },
        None => (LayoutSpec::new("us", None::<String>), None),
    };
    Detection { spec, backend, registry }
}

/// Watch for live layout switches. Calls `on_change` with each NEW layout
/// (deduplicated). Event stream when the backend has one; if the stream dies
/// or was never available, poll every POLL_INTERVAL (spec fallback rule).
pub fn spawn_watcher(
    backend: Backend,
    initial: LayoutSpec,
    registry: Arc<Registry>,
    on_change: impl Fn(LayoutSpec) + Send + 'static,
) {
    std::thread::spawn(move || {
        let mut last = initial;
        if let Some(stream) = backend.watch_stream() {
            for line in stream.lines() {
                if line.is_err() {
                    break; // child/socket died → fall through to polling
                }
                redetect(backend, &registry, &mut last, &on_change);
            }
            eprintln!("kbcut: layout event stream ended, falling back to polling");
        }
        loop {
            std::thread::sleep(POLL_INTERVAL);
            redetect(backend, &registry, &mut last, &on_change);
        }
    });
}

fn redetect(
    backend: Backend,
    registry: &Registry,
    last: &mut LayoutSpec,
    on_change: &impl Fn(LayoutSpec),
) {
    // Detection failure keeps the last good keymap (spec error philosophy).
    if let Ok(spec) = backend.current(registry) {
        if spec != *last {
            *last = spec.clone();
            on_change(spec);
        }
    }
}
```

- [ ] **Step 3: Build + test**

Run: `cargo test`
Expected: PASS (all previous tests; no new unit tests here — thread/stream plumbing is exercised in Task 6's manual check)

- [ ] **Step 4: Commit**

```bash
git add src/layout/
git commit -m "layout: live watch streams and watcher thread with poll fallback"
```

---

### Task 6: Daemon integration — live keymap swap

**Files:**
- Modify: `src/daemon.rs`

- [ ] **Step 1: Wire the layout module into the daemon**

In `src/daemon.rs`:

1. Add imports: `use crate::layout::{self, LayoutSpec};`
2. Add a variant to `enum Msg`: `LayoutChanged(LayoutSpec),`
3. Replace the detection block (lines 39–47, the `let layout = ... let mut keymap = ...` section) with:

```rust
    let detection = layout::init(cfg.layout.as_deref());
    let mut keymap = Keymap::new(
        &detection.spec.layout,
        detection.spec.variant.as_deref().unwrap_or(""),
    )?;
```

4. After `let (tx, rx) = channel::<Msg>();`, spawn the watcher:

```rust
    if let Some(backend) = detection.backend {
        let watch_tx = tx.clone();
        layout::spawn_watcher(
            backend,
            detection.spec.clone(),
            Arc::clone(&detection.registry),
            move |spec| {
                let _ = watch_tx.send(Msg::LayoutChanged(spec));
            },
        );
    }
```

5. Add the message arm in the `for msg in rx` loop (next to `Msg::ReloadConfig`):

```rust
            Msg::LayoutChanged(spec) => {
                match Keymap::new(&spec.layout, spec.variant.as_deref().unwrap_or("")) {
                    Ok(new_keymap) => {
                        keymap = new_keymap;
                        // Never decode one word under two layouts: drop the
                        // buffer AND any expansion waiting for key release.
                        word.clear();
                        pending = None;
                        eprintln!("kbcut: layout switched to '{spec}'");
                    }
                    Err(e) => eprintln!("kbcut: keeping previous layout, switch failed: {e:#}"),
                }
            }
```

6. Delete `detect_gnome_layout` and `gnome_input_source_layout` (lines 354–378) — the logic now lives in `layout::backends` (`parse_gnome_sources`).

- [ ] **Step 2: Build, lint, test**

Run: `cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS, no warnings

- [ ] **Step 3: Manual verification on this machine (GNOME/Wayland)**

```bash
cargo run -- daemon &
sleep 2
# switch layout us ⇄ pt via GNOME (Super+Space), watch stderr:
# expect "kbcut: layout switched to 'pt'" within ~1s of switching
kill %1
```

Expected: startup line `kbcut: layout 'us' via gnome backend` (or pt), switch line on layout change, typing a trigger still expands after the switch.

- [ ] **Step 4: Commit**

```bash
git add src/daemon.rs
git commit -m "daemon: live layout switching via layout watcher"
```

---

### Task 7: Clipboard abstraction

**Files:**
- Create: `src/clipboard.rs`
- Modify: `src/inject.rs`, `src/daemon.rs:50` (Injector construction), `src/main.rs` (module decl)

- [ ] **Step 1: Write `src/clipboard.rs`**

```rust
//! Clipboard access for the paste fallback (characters not typeable on the
//! current layout). Wayland: wl-clipboard. X11: xclip or xsel. Neither →
//! Disabled, and the injector types the typeable subset instead.

use anyhow::{anyhow, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Wayland,
    Xclip,
    Xsel,
    Disabled,
}

impl Backend {
    pub fn detect() -> Self {
        Self::select(
            |k| std::env::var(k).is_ok(),
            |bin| which(bin),
        )
    }

    /// Testable core: env presence + tool presence.
    pub fn select(env_set: impl Fn(&str) -> bool, has: impl Fn(&str) -> bool) -> Self {
        if env_set("WAYLAND_DISPLAY") && has("wl-copy") {
            return Backend::Wayland;
        }
        if env_set("DISPLAY") {
            if has("xclip") {
                return Backend::Xclip;
            }
            if has("xsel") {
                return Backend::Xsel;
            }
        }
        Backend::Disabled
    }

    pub fn available(&self) -> bool {
        *self != Backend::Disabled
    }

    /// One line for startup logs and `kbcut doctor`.
    pub fn describe(&self) -> String {
        match self {
            Backend::Wayland => "wl-clipboard (Wayland)".into(),
            Backend::Xclip => "xclip (X11)".into(),
            Backend::Xsel => "xsel (X11)".into(),
            Backend::Disabled => {
                "none — emoji/special-character replacements will be skipped \
                 (install wl-clipboard on Wayland, or xclip on X11)"
                    .into()
            }
        }
    }

    pub fn get(&self) -> Option<Vec<u8>> {
        let (bin, args): (&str, &[&str]) = match self {
            Backend::Wayland => ("wl-paste", &["--no-newline"]),
            Backend::Xclip => ("xclip", &["-selection", "clipboard", "-o"]),
            Backend::Xsel => ("xsel", &["-b"]),
            Backend::Disabled => return None,
        };
        Command::new(bin)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
    }

    pub fn set(&self, bytes: &[u8]) -> Result<()> {
        let (bin, args): (&str, &[&str]) = match self {
            Backend::Wayland => ("wl-copy", &[]),
            Backend::Xclip => ("xclip", &["-selection", "clipboard"]),
            Backend::Xsel => ("xsel", &["-b", "-i"]),
            Backend::Disabled => return Err(anyhow!("no clipboard tool available")),
        };
        let mut child = Command::new(bin)
            .args(args)
            .stdin(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning {bin}"))?;
        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(bytes)
            .with_context(|| format!("writing to {bin} stdin"))?;
        child.wait().with_context(|| format!("waiting for {bin}"))?;
        Ok(())
    }
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_order() {
        let all = |_: &str| true;
        let none = |_: &str| false;
        assert_eq!(Backend::select(|k| k == "WAYLAND_DISPLAY", all), Backend::Wayland);
        assert_eq!(Backend::select(|k| k == "DISPLAY", all), Backend::Xclip);
        assert_eq!(
            Backend::select(|k| k == "DISPLAY", |b| b == "xsel"),
            Backend::Xsel
        );
        assert_eq!(Backend::select(|_| false, all), Backend::Disabled);
        assert_eq!(Backend::select(|_| true, none), Backend::Disabled);
        // Wayland session without wl-copy but with xclip and DISPLAY (XWayland)
        assert_eq!(Backend::select(|_| true, |b| b == "xclip"), Backend::Xclip);
    }
}
```

Add `mod clipboard;` to `src/main.rs`.

- [ ] **Step 2: Run the new test**

Run: `cargo test --lib clipboard`
Expected: PASS

- [ ] **Step 3: Rewire `src/inject.rs`**

FROZEN BEHAVIOR (spec table): save previous clipboard → set → Ctrl+V → 150ms settle → restore. Only the invoked command changes.

1. Replace `use std::io::Write;` usage: delete the `clipboard_set` / `clipboard_set_bytes` functions (lines 135–152) and the `use std::io::Write;` import.
2. Add `use crate::clipboard;` import.
3. Store the backend:

```rust
pub struct Injector {
    device: VirtualDevice,
    clipboard: clipboard::Backend,
}
```

In `Injector::new`, accept it:

```rust
    pub fn new(clipboard: clipboard::Backend) -> Result<Self> {
        // ... existing body unchanged ...
        Ok(Self { device, clipboard })
    }
```

4. In `replace()`, route around a missing clipboard — typing the typeable subset beats losing the replacement (type_text already warns per skipped char):

```rust
        if fully_typeable || !self.clipboard.available() {
            self.type_text(text, keymap)?;
        } else {
            self.paste_text(text)?;
        }
```

5. Rewrite `paste_text` to use the backend (same sequence, same delays):

```rust
    fn paste_text(&mut self, text: &str) -> Result<()> {
        let previous_clipboard = self.clipboard.get();

        if let Err(e) = self.clipboard.set(text.as_bytes()) {
            eprintln!("kbcut: clipboard set failed, typing what's typeable instead: {e:#}");
            return Ok(());
        }

        self.emit_key(Key::KEY_LEFTCTRL.code(), true)?;
        self.tap(Key::KEY_V.code(), false, false)?;
        self.emit_key(Key::KEY_LEFTCTRL.code(), false)?;
        sleep(PASTE_SETTLE_DELAY);

        if let Some(prev) = previous_clipboard {
            let _ = self.clipboard.set(&prev);
        }
        Ok(())
    }
```

6. In `src/daemon.rs`, update construction and log the backend once (the spec's one-time warning):

```rust
    let clip = clipboard::Backend::detect();
    eprintln!("kbcut: clipboard backend: {}", clip.describe());
    let mut injector = Injector::new(clip)?;
```

(add `use crate::clipboard;` to daemon.rs imports)

- [ ] **Step 4: Build, lint, test, manual check**

Run: `cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS

Manual (GNOME/Wayland box): `cargo run -- daemon`, type `zshrug ` in a text field → `¯\_(ツ)_/¯ ` appears; previous clipboard content restored afterwards.

- [ ] **Step 5: Commit**

```bash
git add src/clipboard.rs src/inject.rs src/daemon.rs src/main.rs
git commit -m "clipboard: backend abstraction with X11 fallback (xclip/xsel)"
```

---

### Task 8: `kbcut setup` — real installer

**Files:**
- Create: `src/setup.rs`
- Modify: `src/main.rs` (Setup flag + dispatch, module decl)

- [ ] **Step 1: Write `src/setup.rs`** (setup half; doctor comes in Task 9)

```rust
//! `kbcut setup` — one-time system install, idempotent and transparent:
//! every privileged command is printed before it runs and confirmed.
//! `kbcut doctor` — diagnose a broken install; the standard bug-report tool.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;
use std::process::Command;

const UDEV_RULE: &str = include_str!("../packaging/99-kbcut-uinput.rules");
const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/99-kbcut-uinput.rules";
const SERVICE_TEMPLATE: &str = include_str!("../packaging/kbcut.service");

pub fn run_setup(print_only: bool) -> Result<()> {
    let bin = std::env::current_exe().context("locating the kbcut binary")?;
    let bin = bin.display().to_string();
    let unit = SERVICE_TEMPLATE.replace("%h/.local/bin/kbcut", &bin);
    let unit_dir = dirs::config_dir().context("no config dir")?.join("systemd/user");
    let unit_path = unit_dir.join("kbcut.service");
    let has_systemd = Command::new("systemctl")
        .args(["--user", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if print_only {
        println!("# 1. udev rule (lets your user use uinput and read input devices)");
        println!("sudo tee {UDEV_RULE_PATH} <<'EOF'\n{}EOF", UDEV_RULE);
        println!("sudo udevadm control --reload-rules && sudo udevadm trigger");
        println!("sudo usermod -aG input $USER");
        println!("# 2. systemd user service");
        println!("mkdir -p {}", unit_dir.display());
        println!("tee {} <<'EOF'\n{}EOF", unit_path.display(), unit);
        println!("systemctl --user daemon-reload && systemctl --user enable kbcut");
        println!("# 3. log out and back in, then: systemctl --user start kbcut");
        println!("# (no systemd? skip step 2 and run `kbcut daemon` from your session autostart)");
        return Ok(());
    }

    // ── udev rule ──────────────────────────────────────────────────────────
    if std::fs::read_to_string(UDEV_RULE_PATH).map(|c| c == UDEV_RULE).unwrap_or(false) {
        println!("✓ udev rule already installed");
    } else if confirm(&format!("Install udev rule to {UDEV_RULE_PATH} (needs sudo)?"))? {
        sudo_write(UDEV_RULE_PATH, UDEV_RULE)?;
        run_visible("sudo", &["udevadm", "control", "--reload-rules"])?;
        run_visible("sudo", &["udevadm", "trigger"])?;
        println!("✓ udev rule installed");
    }

    // ── input group ────────────────────────────────────────────────────────
    if in_group_active("input") {
        println!("✓ user is in the input group");
    } else if in_group_configured("input") {
        println!("✓ input group configured — log out and back in to apply");
    } else {
        let user = std::env::var("USER").unwrap_or_default();
        if confirm(&format!("Add {user} to the input group (needs sudo)?"))? {
            run_visible("sudo", &["usermod", "-aG", "input", &user])?;
            println!("✓ added — log out and back in to apply");
        }
    }

    // ── systemd user service ───────────────────────────────────────────────
    if !has_systemd {
        println!("! no systemd user session detected — run `kbcut daemon` from your session autostart instead");
        return Ok(());
    }
    if std::fs::read_to_string(&unit_path).map(|c| c == unit).unwrap_or(false) {
        println!("✓ systemd unit already installed");
    } else if confirm(&format!("Install systemd user unit to {}?", unit_path.display()))? {
        std::fs::create_dir_all(&unit_dir)?;
        std::fs::write(&unit_path, &unit)?;
        run_visible("systemctl", &["--user", "daemon-reload"])?;
        run_visible("systemctl", &["--user", "enable", "kbcut"])?;
        println!("✓ service installed and enabled");
    }

    println!("\nDone. Log out and back in (group membership applies at login), then:");
    println!("  systemctl --user start kbcut");
    println!("  kbcut doctor   # verify everything");
    Ok(())
}

fn confirm(question: &str) -> Result<bool> {
    print!("{question} [Y/n] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "" | "y" | "Y" | "yes"))
}

fn run_visible(bin: &str, args: &[&str]) -> Result<()> {
    println!("  $ {bin} {}", args.join(" "));
    let status = Command::new(bin).args(args).status().with_context(|| format!("running {bin}"))?;
    anyhow::ensure!(status.success(), "{bin} exited with {status}");
    Ok(())
}

fn sudo_write(path: &str, content: &str) -> Result<()> {
    println!("  $ sudo tee {path}");
    let mut child = Command::new("sudo")
        .args(["tee", path])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .context("spawning sudo tee")?;
    child.stdin.take().expect("piped").write_all(content.as_bytes())?;
    anyhow::ensure!(child.wait()?.success(), "sudo tee failed");
    Ok(())
}

/// Group active in the current session (`id -nG`).
fn in_group_active(group: &str) -> bool {
    Command::new("id")
        .arg("-nG")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.split_whitespace().any(|g| g == group))
        .unwrap_or(false)
}

/// Group configured in /etc/group but not yet active (needs re-login).
fn in_group_configured(group: &str) -> bool {
    let user = std::env::var("USER").unwrap_or_default();
    std::fs::read_to_string("/etc/group")
        .map(|content| {
            content.lines().any(|l| {
                let mut parts = l.split(':');
                parts.next() == Some(group)
                    && parts.nth(2).map(|members| members.split(',').any(|m| m == user)).unwrap_or(false)
            })
        })
        .unwrap_or(false)
}
```

Add `mod setup;` to `src/main.rs`.

- [ ] **Step 2: Wire the CLI**

In `src/main.rs`, change the `Setup` variant and dispatch, and delete `print_setup()`:

```rust
    /// Install udev rule, input group, and systemd user service
    Setup {
        /// Print the commands instead of running them
        #[arg(long)]
        print: bool,
    },
```

```rust
        Command::Setup { print } => setup::run_setup(print)?,
```

- [ ] **Step 3: Build + verify `--print`**

Run: `cargo clippy --all-targets -- -D warnings && cargo run -- setup --print`
Expected: full command listing, no prompts, exit 0. Then run `cargo run -- setup` interactively — every step should report `✓ ... already installed` on this machine (idempotence check).

- [ ] **Step 4: Commit**

```bash
git add src/setup.rs src/main.rs
git commit -m "setup: real installer subcommand with embedded assets"
```

---

### Task 9: `kbcut doctor`

**Files:**
- Modify: `src/setup.rs` (append), `src/main.rs` (subcommand)

- [ ] **Step 1: Append doctor to `src/setup.rs`**

```rust
pub fn run_doctor() -> Result<()> {
    let mut failed = false;
    let mut check = |name: &str, ok: bool, hint: &str| {
        println!("{} {name}", if ok { "✓" } else { "✗" });
        if !ok {
            println!("    → {hint}");
            failed = true;
        }
    };

    check(
        "uinput kernel module",
        Path::new("/sys/class/misc/uinput").exists(),
        "sudo modprobe uinput (and check it's not blacklisted)",
    );
    check(
        "/dev/uinput writable",
        std::fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok(),
        "run `kbcut setup` to install the udev rule, then log out and back in",
    );
    let active = in_group_active("input");
    let configured = in_group_configured("input");
    check(
        "input group membership",
        active,
        if configured {
            "group is configured but not active — log out and back in"
        } else {
            "run `kbcut setup` (adds you to the input group)"
        },
    );
    check(
        "udev rule installed",
        Path::new(UDEV_RULE_PATH).exists(),
        "run `kbcut setup`",
    );
    let readable_devices = evdev::enumerate().count();
    check(
        &format!("input devices readable ({readable_devices})"),
        readable_devices > 0,
        "needs the input group active — log out and back in after setup",
    );

    match crate::config::load() {
        Ok(cfg) => check(
            &format!("config parses ({} replacements)", cfg.replacements.len()),
            true,
            "",
        ),
        Err(e) => check("config parses", false, &format!("{e:#}")),
    }

    let cfg_layout = crate::config::load().ok().and_then(|c| c.layout);
    let detection = crate::layout::init(cfg_layout.as_deref());
    let source = match detection.backend {
        Some(b) => format!("{} backend", b.name()),
        None => "config override".to_string(),
    };
    check(&format!("layout: '{}' via {source}", detection.spec), true, "");

    let clip = crate::clipboard::Backend::detect();
    check(
        &format!("clipboard: {}", clip.describe()),
        clip.available(),
        "install wl-clipboard (Wayland) or xclip (X11) for emoji/special-char replacements",
    );

    let service = Command::new("systemctl")
        .args(["--user", "is-active", "kbcut"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown (no systemd)".into());
    check(
        &format!("service: {service}"),
        service == "active",
        "systemctl --user start kbcut (or run `kbcut daemon` manually)",
    );

    if failed {
        anyhow::bail!("some checks failed — see hints above");
    }
    println!("\nAll checks passed.");
    Ok(())
}
```

- [ ] **Step 2: Wire the CLI**

In `src/main.rs`:

```rust
    /// Check the installation and environment, with fix hints
    Doctor,
```

```rust
        Command::Doctor => setup::run_doctor()?,
```

- [ ] **Step 3: Run it**

Run: `cargo clippy --all-targets -- -D warnings && cargo run -- doctor`
Expected on this machine: all checks ✓ except possibly `service` (✗ if not currently running is fine — verify the hint prints and exit code is non-zero: `echo $?` → not 0). Then `systemctl --user start kbcut 2>/dev/null; cargo run -- doctor; echo $?` → 0 if everything is green.

- [ ] **Step 4: Commit**

```bash
git add src/setup.rs src/main.rs
git commit -m "doctor: install/environment diagnostics with fix hints"
```

---

### Task 10: Release hygiene — LICENSE, CHANGELOG, CI, Cargo metadata

**Files:**
- Create: `LICENSE`, `CHANGELOG.md`, `.github/workflows/ci.yml`
- Modify: `Cargo.toml`

- [ ] **Step 1: LICENSE (MIT)**

Standard MIT text with the line `Copyright (c) 2026 Hugo Macedo`.

- [ ] **Step 2: CHANGELOG.md**

```markdown
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
```

- [ ] **Step 3: `.github/workflows/ci.yml`**

```yaml
name: CI
on:
  push: { branches: [master] }
  pull_request:
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: sudo apt-get update && sudo apt-get install -y libxkbcommon-dev
      - uses: dtolnay/rust-toolchain@stable
        with: { components: "rustfmt, clippy" }
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test
```

- [ ] **Step 4: Cargo.toml metadata for crates.io**

```toml
[package]
name = "kbcut"
version = "0.2.0"
edition = "2021"
description = "Seamless text replacement for Linux (Wayland-friendly), like macOS text shortcuts"
license = "MIT"
repository = "https://github.com/hhpmacedo/kbcut"
readme = "README.md"
keywords = ["text-expansion", "wayland", "uinput", "keyboard", "snippets"]
categories = ["command-line-utilities"]
```

- [ ] **Step 5: Verify fmt is clean, fix if not**

Run: `cargo fmt --check || cargo fmt`  (then re-run tests if fmt changed files)
Run: `cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add LICENSE CHANGELOG.md .github/ Cargo.toml Cargo.lock
git commit -m "release hygiene: LICENSE, CHANGELOG, CI, crates.io metadata"
```

---

### Task 11: README rewrite

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Rewrite README.md**

Keep the voice of the current README (direct, honest about limitations). New structure — write it fully, adapting the existing text:

1. **Intro** — unchanged pitch (`brb` → `be right back`, every app, Wayland included).
2. **Install** — replaces "built for a specific machine setup":

```markdown
## Install

    cargo install kbcut
    kbcut setup      # udev rule, input group, systemd service — asks before each step
    kbcut doctor     # verify

Log out and back in (group membership applies at login).

Requires Linux with the `uinput` module (default on virtually every distro).
The background service assumes a `systemd --user` session; without one, run
`kbcut daemon` from your session autostart. `kbcut setup --print` shows the
commands without running them.
```

3. **How it works** — keep current text verbatim.
4. **Usage** — keep current text (`add`/`rm`/`list`, config path, auto-reload). Add: prefer `kbcut add` over hand-editing TOML (it escapes special characters correctly).
5. **Layout detection** — new section: detected automatically on GNOME, KDE, Sway, Hyprland; generic X11 and `localectl` fallbacks; **live-tracks layout switching**; `layout = "pt"` or `layout = "pt(nativo)"` in the config pins it and disables detection (first thing to try if expansions produce wrong characters).
6. **Special characters** — clipboard paste via wl-clipboard (Wayland) or xclip/xsel (X11); if neither is installed those characters are skipped with a startup warning.
7. **Known limitations** — carry over: Enter/Tab don't expand (deliberate), Caps Lock case inversion, X11 active-layout is first-configured + polled. Drop the ones that are now fixed (restart-after-switch, X11 silent skip).
8. **Troubleshooting** — `kbcut doctor` first; config `layout` override; `journalctl --user -u kbcut -f`.

- [ ] **Step 2: Sanity-check rendered markdown**

Run: `grep -n "specific machine" README.md`
Expected: no matches (the old framing is gone)

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "README: rewrite for public release"
```

---

### Task 12: Publish 0.2.0

**Files:** none (release mechanics)

- [ ] **Step 1: Full check + dry-run**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo publish --dry-run
```
Expected: dry-run packages successfully (warnings about uncommitted files mean a missed `git add` — fix first).

- [ ] **Step 2: Push and verify CI**

```bash
git push origin master
gh run watch --repo hhpmacedo/kbcut
```
Expected: CI green. If red, fix before proceeding.

- [ ] **Step 3: Manual QA checklist (spec §Testing)**

On this machine (GNOME/Wayland): `kbcut doctor` all green; `brb ` expands; `zshrug ` pastes; us ⇄ pt switch mid-session then `brb ` still expands.
Document results in the PR/commit message. (Sway and X11 environments: best-effort, test in a VM or note as untested-by-hand — CI covers their parsers.)

- [ ] **Step 4: Tag and publish** — ⚠️ STOP: get explicit user confirmation before `cargo publish` (irreversible: a crates.io version can be yanked but never replaced).

```bash
git tag v0.2.0 && git push origin v0.2.0
cargo publish
```

- [ ] **Step 5: Verify**

Run: `cargo search kbcut`
Expected: `kbcut = "0.2.0"` listed.

---

## Self-review notes

- **Spec coverage:** layout backends+registry (T3–T5), active-not-first fixtures (T4), live tracking + word/pending clear (T6), clipboard + one-time warning (T7), setup/doctor + embedded assets + `--print` + no-systemd path (T8–T9), config robustness + TOML regression tests (T2), variant support (T1), release hygiene + CI + publish (T10–T12). Frozen behaviors: T7 changes only the invoked command; the `Pending` machine is untouched by any task.
- **Deviation from spec (intentional):** the spec sketched `Arc<RwLock<Keymap>>` for the swap; the plan uses `Msg::LayoutChanged` through the daemon's existing mpsc channel instead — same requirement (swap without restructuring the loop, clear word+pending), zero locking, single owner. Registry is `Arc` shared with the watcher thread as specced.
- **Type consistency:** `LayoutSpec {layout, variant}` defined T3, used T4–T6, T9; `clipboard::Backend` defined T7, used T7/T9; `Keymap::new(layout, variant)` changed T1, used T6.
