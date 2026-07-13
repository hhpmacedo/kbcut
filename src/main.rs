mod config;
mod daemon;
mod inject;
mod keymap;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "kbcut", version, about = "Seamless text replacement, macOS-style")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Add or update a replacement: kbcut add brb "be right back"
    Add { trigger: String, replacement: String },
    /// Remove a replacement
    Rm { trigger: String },
    /// List all replacements
    List,
    /// Run the expansion daemon (foreground)
    Daemon,
    /// Print the one-time system setup instructions
    Setup,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Add { trigger, replacement } => {
            if trigger.chars().any(|c| c.is_whitespace()) {
                anyhow::bail!("trigger cannot contain whitespace");
            }
            let mut cfg = config::load()?;
            cfg.replacements.insert(trigger.clone(), replacement.clone());
            config::save(&cfg)?;
            println!("{trigger} → {replacement}");
        }
        Command::Rm { trigger } => {
            let mut cfg = config::load()?;
            if cfg.replacements.remove(&trigger).is_some() {
                config::save(&cfg)?;
                println!("removed {trigger}");
            } else {
                println!("no such trigger: {trigger}");
            }
        }
        Command::List => {
            let cfg = config::load()?;
            if cfg.replacements.is_empty() {
                println!("no replacements defined — add one with: kbcut add brb \"be right back\"");
            }
            for (trigger, replacement) in &cfg.replacements {
                println!("{trigger} → {replacement}");
            }
        }
        Command::Daemon => daemon::run()?,
        Command::Setup => print_setup(),
    }
    Ok(())
}

fn print_setup() {
    println!(
        r#"One-time system setup (run from the repo root):

  # 1. Let your user read input devices and write to uinput
  sudo cp packaging/99-kbcut-uinput.rules /etc/udev/rules.d/
  sudo udevadm control --reload-rules && sudo udevadm trigger
  sudo usermod -aG input $USER

  # 2. Install the binary and the systemd user service
  cargo build --release
  mkdir -p ~/.local/bin ~/.config/systemd/user
  cp target/release/kbcut ~/.local/bin/
  cp packaging/kbcut.service ~/.config/systemd/user/
  systemctl --user daemon-reload
  systemctl --user enable kbcut

  # 3. Log out and back in (group membership takes effect at login),
  #    then the service starts automatically. To start it right away:
  systemctl --user start kbcut
"#
    );
}
