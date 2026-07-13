use anyhow::{Context, Result};
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, EventType, InputEvent, Key};
use std::thread::sleep;
use std::time::Duration;

use crate::clipboard;
use crate::keymap::Keymap;

pub const VIRTUAL_DEVICE_NAME: &str = "kbcut virtual keyboard";

/// Time to let the target app finish reading the clipboard after a paste,
/// before we restore whatever was on it beforehand.
const PASTE_SETTLE_DELAY: Duration = Duration::from_millis(150);

/// Delay between injected key events. Some apps drop events that arrive
/// faster than a human could type.
const KEY_DELAY: Duration = Duration::from_millis(2);
/// Grace period before injecting, so the compositor and focused app finish
/// processing the physical keystroke that triggered the expansion. Injection
/// already waits for the boundary key's release, so this can stay short.
const PRE_INJECT_DELAY: Duration = Duration::from_millis(10);

pub struct Injector {
    device: VirtualDevice,
    clipboard: clipboard::Backend,
}

impl Injector {
    pub fn new(clipboard: clipboard::Backend) -> Result<Self> {
        let mut keys = AttributeSet::<Key>::new();
        // Register every key the reverse keymap could ask for.
        for code in 0..=255u16 {
            keys.insert(Key::new(code));
        }
        let device = VirtualDeviceBuilder::new()
            .context("opening /dev/uinput (is the udev rule installed and are you in the `input` group?)")?
            .name(VIRTUAL_DEVICE_NAME)
            .with_keys(&keys)?
            .build()?;
        Ok(Self { device, clipboard })
    }

    fn emit_key(&mut self, code: u16, pressed: bool) -> Result<()> {
        let value = if pressed { 1 } else { 0 };
        self.device.emit(&[
            InputEvent::new(EventType::KEY, code, value),
            InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
        ])?;
        sleep(KEY_DELAY);
        Ok(())
    }

    fn tap(&mut self, code: u16, shift: bool, altgr: bool) -> Result<()> {
        if shift {
            self.emit_key(Key::KEY_LEFTSHIFT.code(), true)?;
        }
        if altgr {
            self.emit_key(Key::KEY_RIGHTALT.code(), true)?;
        }
        self.emit_key(code, true)?;
        self.emit_key(code, false)?;
        if altgr {
            self.emit_key(Key::KEY_RIGHTALT.code(), false)?;
        }
        if shift {
            self.emit_key(Key::KEY_LEFTSHIFT.code(), false)?;
        }
        Ok(())
    }

    /// Erase `count` characters, then type `text`. Falls back to a clipboard
    /// paste when `text` contains a character not reachable on the current
    /// keyboard layout (e.g. emoji or symbols that need a Unicode input
    /// method rather than a key combo).
    pub fn replace(&mut self, count: usize, text: &str, keymap: &Keymap) -> Result<()> {
        sleep(PRE_INJECT_DELAY);
        for _ in 0..count {
            self.tap(Key::KEY_BACKSPACE.code(), false, false)?;
        }
        let fully_typeable = text
            .chars()
            .all(|c| matches!(c, '\n' | '\t') || keymap.combo_for(c).is_some());
        if fully_typeable || !self.clipboard.available() {
            self.type_text(text, keymap)?;
        } else {
            self.paste_text(text)?;
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str, keymap: &Keymap) -> Result<()> {
        for c in text.chars() {
            if c == '\n' {
                self.tap(Key::KEY_ENTER.code(), false, false)?;
                continue;
            }
            if c == '\t' {
                self.tap(Key::KEY_TAB.code(), false, false)?;
                continue;
            }
            match keymap.combo_for(c) {
                Some(combo) => self.tap(combo.keycode, combo.shift, combo.altgr)?,
                None => eprintln!("kbcut: cannot type {c:?} on the current layout, skipping"),
            }
        }
        Ok(())
    }

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
}
