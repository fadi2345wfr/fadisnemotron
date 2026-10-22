//! Keeps the Mac from falling asleep while the phone or an agent run needs it.
//!
//! macOS gives a program no way to stay awake with the lid shut except for
//! `pmset disablesleep`, which needs the administrator password; that is a
//! button the user presses on purpose, never something done silently.

use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// How awake the Mac should stay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Awake {
    /// Sleep as usual.
    No,
    /// The Mac stays awake, the screen may go dark (phone connected).
    System,
    /// The screen stays on too: an agent run may look at it.
    Screen,
}

/// A running `caffeinate`, ended when the level drops or the app quits.
pub struct KeepAwake {
    level: Awake,
    child: Option<Child>,
    /// `caffeinate` could not be started; asking again every frame would not help.
    gave_up: bool,
}

impl KeepAwake {
    pub fn new() -> Self {
        Self {
            level: Awake::No,
            child: None,
            gave_up: false,
        }
    }

    #[cfg(test)]
    pub fn level(&self) -> Awake {
        self.level
    }

    /// Switches to `level`; call it every frame, it only acts on changes
    /// (and when `caffeinate` was ended from outside).
    pub fn set(&mut self, level: Awake) {
        let alive = self
            .child
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_none());
        if level == self.level && (alive || level == Awake::No || self.gave_up) {
            return;
        }
        self.stop();
        self.level = level;
        self.gave_up = false;
        if level != Awake::No && cfg!(target_os = "macos") {
            self.child = Command::new("caffeinate")
                .args(caffeinate_args(level, std::process::id()))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok();
            self.gave_up = self.child.is_none();
        } else {
            self.gave_up = true;
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `-w` ties it to this program: if the app crashes, the Mac may sleep again.
fn caffeinate_args(level: Awake, pid: u32) -> Vec<String> {
    let mut args = match level {
        Awake::Screen => vec!["-d", "-i", "-s"],
        _ => vec!["-i", "-s"],
    }
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    args.push("-w".into());
    args.push(pid.to_string());
    args
}

/// Whether sleep is switched off system-wide (also with the lid shut).
pub fn lid_sleep_disabled() -> Option<bool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = Command::new("pmset").arg("-g").output().ok()?;
    parse_sleep_disabled(&String::from_utf8_lossy(&out.stdout))
}

fn parse_sleep_disabled(pmset: &str) -> Option<bool> {
    pmset.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next()? == "SleepDisabled").then(|| words.next() == Some("1"))
    })
}

/// Asks for the administrator password and switches system-wide sleep off
/// (`true`) or back on (`false`). The answer arrives on the channel.
pub fn set_lid_sleep_disabled(disabled: bool) -> Receiver<Result<(), String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_pmset(disabled));
    });
    rx
}

fn run_pmset(disabled: bool) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Das geht nur auf dem Mac.".into());
    }
    // Fixed text, no value from outside goes into the script.
    let script = if disabled {
        "do shell script \"/usr/bin/pmset -a disablesleep 1\" with administrator privileges with prompt \"Fadis Nemotron soll den Mac auch bei zugeklapptem Deckel wach halten.\""
    } else {
        "do shell script \"/usr/bin/pmset -a disablesleep 0\" with administrator privileges with prompt \"Fadis Nemotron soll den Mac wieder normal schlafen lassen.\""
    };
    let mut child = Command::new("osascript")
        .args(["-e", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("osascript konnte nicht gestartet werden: {e}"))?;
    // The user has time to type the password, but not forever.
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < Duration::from_secs(180) => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Zeit abgelaufen: das Passwort wurde nicht eingegeben.".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("-128") {
        Err("Abgebrochen.".into())
    } else {
        Err(format!("Der Mac hat das abgelehnt: {}", stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caffeinate_is_tied_to_the_app_and_the_screen_only_when_needed() {
        assert_eq!(caffeinate_args(Awake::System, 42), ["-i", "-s", "-w", "42"]);
        assert_eq!(
            caffeinate_args(Awake::Screen, 7),
            ["-d", "-i", "-s", "-w", "7"]
        );
    }

    #[test]
    fn pmset_output_is_read() {
        let on =
            "System-wide power settings:\n SleepDisabled\t\t1\nCurrently in use:\n standby 1\n";
        let off = "System-wide power settings:\n SleepDisabled\t\t0\n";
        assert_eq!(parse_sleep_disabled(on), Some(true));
        assert_eq!(parse_sleep_disabled(off), Some(false));
        // Older systems do not list it.
        assert_eq!(parse_sleep_disabled("standby 1\n"), None);
    }

    #[test]
    fn levels_change_without_a_mac() {
        let mut awake = KeepAwake::new();
        assert_eq!(awake.level(), Awake::No);
        awake.set(Awake::Screen);
        assert_eq!(awake.level(), Awake::Screen);
        awake.set(Awake::No);
        assert_eq!(awake.level(), Awake::No);
    }
}
