mod clipboard;
mod config;
mod daemon;
mod inject;
mod keymap;
mod layout;
mod setup;

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
    /// Install udev rule, input group, and systemd user service
    Setup {
        /// Print the commands instead of running them
        #[arg(long)]
        print: bool,
    },
    /// Check the installation and environment, with fix hints
    Doctor,
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
        Command::Setup { print } => setup::run_setup(print)?,
        Command::Doctor => setup::run_doctor()?,
    }
    Ok(())
}
