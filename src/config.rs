use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// xkb layout used to translate keycodes to characters (e.g. "us", "pt").
    /// Autodetected via the desktop's own tooling (GNOME/KDE/Sway/Hyprland,
    /// with X11/localectl fallbacks) when unset; setting this pins the
    /// layout and disables detection.
    #[serde(default)]
    pub layout: Option<String>,

    /// trigger -> replacement
    #[serde(default)]
    pub replacements: BTreeMap<String, String>,
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("kbcut")
        .join("config.toml")
}

pub fn load() -> Result<Config> {
    let path = config_path();
    if !path.exists() {
        return Ok(Config::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

pub fn save(config: &Config) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = toml::to_string_pretty(config)?;
    std::fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

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
