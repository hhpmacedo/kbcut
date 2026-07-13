use anyhow::{anyhow, Result};
use std::collections::HashMap;
use xkbcommon::xkb;

/// How to produce a character on the virtual keyboard: which evdev keycode
/// to press and which modifiers must be held.
#[derive(Debug, Clone, Copy)]
pub struct KeyCombo {
    pub keycode: u16, // evdev keycode (xkb keycode - 8)
    pub shift: bool,
    pub altgr: bool,
}

pub struct Keymap {
    pub state: xkb::State,
    reverse: HashMap<char, KeyCombo>,
}

impl Keymap {
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

    /// Track a hardware key event so shift/caps/altgr state stays accurate.
    pub fn update(&mut self, evdev_code: u16, pressed: bool) {
        let direction = if pressed { xkb::KeyDirection::Down } else { xkb::KeyDirection::Up };
        self.state
            .update_key(xkb::Keycode::new(evdev_code as u32 + 8), direction);
    }

    /// Character produced by a key press under the current modifier state,
    /// if it produces exactly one printable character.
    pub fn char_for(&self, evdev_code: u16) -> Option<char> {
        let utf8 = self
            .state
            .key_get_utf8(xkb::Keycode::new(evdev_code as u32 + 8));
        let mut chars = utf8.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if !c.is_control() => Some(c),
            _ => None,
        }
    }

    /// How to type a character on this layout, if possible.
    pub fn combo_for(&self, c: char) -> Option<KeyCombo> {
        self.reverse.get(&c).copied()
    }

    pub fn is_modifier_active(&self) -> bool {
        for name in [xkb::MOD_NAME_SHIFT, xkb::MOD_NAME_CTRL, xkb::MOD_NAME_ALT, xkb::MOD_NAME_LOGO] {
            if self
                .state
                .mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE)
            {
                return true;
            }
        }
        false
    }

    /// True when a chord modifier (Ctrl/Alt/Super) is held — keystrokes are
    /// commands, not text.
    pub fn is_shortcut_modifier_active(&self) -> bool {
        for name in [xkb::MOD_NAME_CTRL, xkb::MOD_NAME_ALT, xkb::MOD_NAME_LOGO] {
            if self
                .state
                .mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE)
            {
                return true;
            }
        }
        false
    }
}

/// Build char -> (keycode, modifiers) by scanning every key at shift levels
/// 0..=3 for the first layout. Levels follow the xkb convention:
/// 0 = plain, 1 = Shift, 2 = AltGr, 3 = Shift+AltGr.
fn build_reverse_map(keymap: &xkb::Keymap) -> HashMap<char, KeyCombo> {
    let mut map: HashMap<char, KeyCombo> = HashMap::new();
    let layout = xkb::LayoutIndex::from(0u32);
    for raw in 8u32..=255 {
        let keycode = xkb::Keycode::new(raw);
        let num_levels = keymap.num_levels_for_key(keycode, layout);
        for level in 0..num_levels.min(4) {
            let syms = keymap.key_get_syms_by_level(keycode, layout, level);
            for sym in syms {
                let cp = xkb::keysym_to_utf32(*sym);
                if cp == 0 {
                    continue;
                }
                let Some(c) = char::from_u32(cp) else { continue };
                if c.is_control() {
                    continue;
                }
                // Prefer the lowest level (fewest modifiers) for each char.
                map.entry(c).or_insert(KeyCombo {
                    keycode: (raw - 8) as u16,
                    shift: level == 1 || level == 3,
                    altgr: level == 2 || level == 3,
                });
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_and_letters_are_typeable() {
        let km = Keymap::new("us", "").unwrap();
        for c in ['b', 'e', ' ', '!', 'A'] {
            let combo = km.combo_for(c);
            println!("{c:?} -> {combo:?}");
            assert!(combo.is_some(), "no combo for {c:?}");
        }
        let space = km.combo_for(' ').unwrap();
        assert_eq!(space.keycode, 57, "space should be evdev KEY_SPACE (57)");
    }

    #[test]
    fn variant_compiles_and_changes_map() {
        // "us" vs "us(intl)": intl has dead keys; plain "us" must still work
        // through the two-arg signature.
        let plain = Keymap::new("us", "").unwrap();
        assert!(plain.combo_for('a').is_some());
        let intl = Keymap::new("us", "intl").unwrap();
        assert!(intl.combo_for('a').is_some());
    }
}
