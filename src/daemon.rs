use anyhow::{Context, Result};
use evdev::{Device, EventType, InputEventKind, Key};
use notify::{RecursiveMode, Watcher};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::config;
use crate::inject::{Injector, VIRTUAL_DEVICE_NAME};
use crate::keymap::Keymap;

const MAX_WORD_LEN: usize = 64;
const RESCAN_INTERVAL: Duration = Duration::from_secs(10);

enum Msg {
    Key { code: u16, value: i32 },
    PointerButton,
    ReloadConfig,
    DeviceGone(PathBuf),
    NewDevice(PathBuf),
}

/// A matched expansion waiting for the boundary key and all modifiers to be
/// physically released before we inject. The compositor drops an injected
/// press of a keycode that is already held on the real keyboard, so typing
/// a space while the user's space key is still down would silently vanish.
struct Pending {
    erase: usize,
    text: String,
    boundary_code: u16,
    boundary_released: bool,
}

pub fn run() -> Result<()> {
    let cfg = config::load()?;
    let layout = cfg
        .layout
        .clone()
        .or_else(detect_gnome_layout)
        .unwrap_or_else(|| "us".to_string());
    eprintln!("kbcut: using xkb layout '{layout}'");

    let mut keymap = Keymap::new(&layout)?;
    // Create the virtual device before enumerating so we can skip our own
    // event node by name.
    let mut injector = Injector::new()?;
    let mut triggers = cfg.replacements.clone();
    eprintln!("kbcut: {} replacement(s) loaded", triggers.len());

    let (tx, rx) = channel::<Msg>();
    let open_paths: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));

    for (path, device) in enumerate_devices() {
        spawn_reader(path, device, tx.clone(), &open_paths);
    }
    if open_paths.lock().unwrap().is_empty() {
        anyhow::bail!(
            "no keyboard devices readable under /dev/input — are you in the `input` group? \
             (run `kbcut setup`, then log out and back in)"
        );
    }

    spawn_rescan(tx.clone(), Arc::clone(&open_paths));
    let _watcher = spawn_config_watcher(tx.clone())?;

    let mut word = String::new();
    let mut pending: Option<Pending> = None;

    for msg in rx {
        match msg {
            Msg::ReloadConfig => match config::load() {
                Ok(new_cfg) => {
                    triggers = new_cfg.replacements;
                    eprintln!("kbcut: config reloaded, {} replacement(s)", triggers.len());
                }
                Err(e) => eprintln!("kbcut: config reload failed: {e:#}"),
            },
            Msg::PointerButton => {
                word.clear();
                pending = None;
            }
            Msg::DeviceGone(path) => {
                open_paths.lock().unwrap().remove(&path);
            }
            Msg::NewDevice(path) => {
                if let Ok(device) = Device::open(&path) {
                    if is_ours(&device) {
                        continue;
                    }
                    spawn_reader(path, device, tx.clone(), &open_paths);
                }
            }
            Msg::Key { code, value } => {
                handle_key(
                    code,
                    value,
                    &mut keymap,
                    &mut injector,
                    &triggers,
                    &mut word,
                    &mut pending,
                );
            }
        }
    }
    Ok(())
}

fn handle_key(
    code: u16,
    value: i32,
    keymap: &mut Keymap,
    injector: &mut Injector,
    triggers: &BTreeMap<String, String>,
    word: &mut String,
    pending: &mut Option<Pending>,
) {
    // Keep xkb modifier/caps state in sync (0 = release, 1 = press).
    if value == 0 || value == 1 {
        keymap.update(code, value == 1);
    }

    if value == 0 {
        // A pending expansion fires once the boundary key itself and every
        // modifier have been released.
        if let Some(p) = pending.as_mut() {
            if code == p.boundary_code {
                p.boundary_released = true;
            }
            if p.boundary_released && !keymap.is_modifier_active() {
                let p = pending.take().unwrap();
                fire(injector, keymap, p);
                word.clear();
            }
        }
        return;
    }

    // From here on: press (1) or autorepeat (2).
    let key = Key::new(code);
    if is_modifier_key(key) {
        return;
    }

    // Keys arriving while an expansion waits for release: fast typists press
    // the next key before releasing space (rollover). Fold printable chars
    // into the expansion — erase them too and retype them after the
    // replacement. Anything non-printable aborts the expansion.
    if let Some(p) = pending.as_mut() {
        if key != Key::KEY_BACKSPACE && !keymap.is_shortcut_modifier_active() {
            if let Some(c) = keymap.char_for(code) {
                p.erase += 1;
                p.text.push(c);
                return;
            }
        }
        *pending = None;
        word.clear();
        return;
    }

    // Ctrl/Alt/Super chords are commands, not text.
    if keymap.is_shortcut_modifier_active() {
        word.clear();
        return;
    }

    if key == Key::KEY_BACKSPACE {
        word.pop();
        return;
    }

    let Some(c) = keymap.char_for(code) else {
        // Arrows, Esc, Enter, F-keys… the cursor likely moved.
        word.clear();
        return;
    };

    if is_boundary(c) {
        if let Some(replacement) = triggers.get(word.as_str()) {
            *pending = Some(Pending {
                erase: word.chars().count() + 1,
                text: format!("{replacement}{c}"),
                boundary_code: code,
                boundary_released: false,
            });
        }
        word.clear();
    } else {
        if word.chars().count() >= MAX_WORD_LEN {
            word.clear();
        }
        word.push(c);
    }
}

fn fire(injector: &mut Injector, keymap: &Keymap, p: Pending) {
    if let Err(e) = injector.replace(p.erase, &p.text, keymap) {
        eprintln!("kbcut: injection failed: {e:#}");
    }
}

/// Word boundaries, macOS-style: whitespace plus sentence punctuation.
/// Characters like - _ @ # remain part of a word so triggers can use them.
fn is_boundary(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '.' | ',' | ';' | ':' | '!' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '…'
        )
}

fn is_modifier_key(key: Key) -> bool {
    matches!(
        key,
        Key::KEY_LEFTSHIFT
            | Key::KEY_RIGHTSHIFT
            | Key::KEY_LEFTCTRL
            | Key::KEY_RIGHTCTRL
            | Key::KEY_LEFTALT
            | Key::KEY_RIGHTALT
            | Key::KEY_LEFTMETA
            | Key::KEY_RIGHTMETA
            | Key::KEY_CAPSLOCK
            | Key::KEY_NUMLOCK
    )
}

fn is_keyboard(device: &Device) -> bool {
    device
        .supported_keys()
        .map(|keys| keys.contains(Key::KEY_A) && keys.contains(Key::KEY_SPACE))
        .unwrap_or(false)
}

fn is_pointer(device: &Device) -> bool {
    device
        .supported_keys()
        .map(|keys| keys.contains(Key::BTN_LEFT))
        .unwrap_or(false)
}

fn is_ours(device: &Device) -> bool {
    device.name() == Some(VIRTUAL_DEVICE_NAME)
}

fn enumerate_devices() -> Vec<(PathBuf, Device)> {
    evdev::enumerate()
        .filter(|(_, d)| !is_ours(d) && (is_keyboard(d) || is_pointer(d)))
        .collect()
}

fn spawn_reader(
    path: PathBuf,
    mut device: Device,
    tx: Sender<Msg>,
    open_paths: &Arc<Mutex<HashSet<PathBuf>>>,
) {
    let keyboard = is_keyboard(&device);
    let pointer = is_pointer(&device);
    if !keyboard && !pointer {
        return;
    }
    {
        let mut paths = open_paths.lock().unwrap();
        if !paths.insert(path.clone()) {
            return; // already reading this device
        }
    }
    eprintln!(
        "kbcut: listening on {} ({})",
        path.display(),
        device.name().unwrap_or("?")
    );
    thread::spawn(move || {
        loop {
            let events = match device.fetch_events() {
                Ok(events) => events,
                Err(_) => break, // device unplugged
            };
            for event in events {
                if event.event_type() != EventType::KEY {
                    continue;
                }
                let msg = match event.kind() {
                    InputEventKind::Key(k)
                        if pointer && matches!(k, Key::BTN_LEFT | Key::BTN_RIGHT | Key::BTN_MIDDLE) =>
                    {
                        if event.value() == 1 {
                            Msg::PointerButton
                        } else {
                            continue;
                        }
                    }
                    InputEventKind::Key(_) if keyboard => Msg::Key {
                        code: event.code(),
                        value: event.value(),
                    },
                    _ => continue,
                };
                if tx.send(msg).is_err() {
                    return;
                }
            }
        }
        let _ = tx.send(Msg::DeviceGone(path));
    });
}

fn spawn_rescan(tx: Sender<Msg>, open_paths: Arc<Mutex<HashSet<PathBuf>>>) {
    thread::spawn(move || loop {
        thread::sleep(RESCAN_INTERVAL);
        let Ok(entries) = std::fs::read_dir("/dev/input") else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name_ok = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("event"))
                .unwrap_or(false);
            if !name_ok || open_paths.lock().unwrap().contains(&path) {
                continue;
            }
            if tx.send(Msg::NewDevice(path)).is_err() {
                return;
            }
        }
    });
}

fn spawn_config_watcher(tx: Sender<Msg>) -> Result<notify::RecommendedWatcher> {
    let path = config::config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap().to_path_buf();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            if event.paths.iter().any(|p| p.ends_with("config.toml")) {
                let _ = tx.send(Msg::ReloadConfig);
            }
        }
    })?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .context("watching config directory")?;
    Ok(watcher)
}

/// First xkb layout configured in GNOME, e.g. [('xkb', 'us'), ('xkb', 'pt')] -> "us".
fn detect_gnome_layout() -> Option<String> {
    // mru-sources tracks actual switch order, its first entry is the layout
    // in effect right now. `sources` is only the configured list order and
    // does not change when the user switches layouts, so it can't tell us
    // which one is active.
    for key in ["mru-sources", "sources"] {
        if let Some(layout) = gnome_input_source_layout(key) {
            return Some(layout);
        }
    }
    None
}

fn gnome_input_source_layout(key: &str) -> Option<String> {
    let out = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.input-sources", key])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let start = text.find("('xkb', '")? + "('xkb', '".len();
    let rest = &text[start..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}
