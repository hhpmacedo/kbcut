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
