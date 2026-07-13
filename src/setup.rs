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
