use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// xkb layout used to translate keycodes to characters (e.g. "us", "pt").
    /// Defaults to the first GNOME input source, falling back to "us".
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
