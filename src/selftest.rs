//! `fadis-nemotron --selftest`: tries the parts of the program that depend on
//! the system (shell, process kill, screen, mouse, clipboard, embedded Android
//! app) and prints what worked. The build on Windows runs it before it
//! publishes; it is also what to run when something does not work on a PC.
//!
//! The exit code is 0 if everything that must work did. What needs a desktop
//! session (screen, mouse, clipboard, Notepad) is reported but does not fail
//! the run, since a build server may have none.

use std::io::Write as _;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::tools::Workspace;

pub const ARG: &str = "--selftest";

struct Report {
    ok: bool,
    lines: Vec<String>,
}

impl Report {
    fn must(&mut self, good: bool, what: impl std::fmt::Display) {
        self.ok &= good;
        self.line(if good { "ok  " } else { "FAIL" }, what);
    }

    fn info(&mut self, what: impl std::fmt::Display) {
        self.line("info", what);
    }

    fn line(&mut self, tag: &str, what: impl std::fmt::Display) {
        let line = format!("[{tag}] {what}");
        println!("{line}");
        self.lines.push(line);
    }
}

fn run(ws: &Workspace, tool: &str, args: Value) -> crate::tools::ToolOutput {
    ws.execute(tool, &args.to_string(), &AtomicBool::new(false))
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .chars()
        .take(160)
        .collect()
}

/// Runs the checks; `true` if all that must work did.
pub fn run_all() -> bool {
    let mut r = Report {
        ok: true,
        lines: Vec::new(),
    };
    r.info(format!(
        "Fadis Nemotron {} auf {} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));

    // The Android app inside the program.
    let expect_apk = std::env::var_os("FADIS_SELFTEST_EXPECT_APK").is_some();
    match crate::remote::bundled_app_file() {
        Some(path) => {
            let size = std::fs::metadata(&path).map_or(0, |m| m.len());
            r.must(
                size > 1_000_000,
                format!("Android-App gefunden: {} ({size} Bytes)", path.display()),
            );
        }
        None => r.must(!expect_apk, "Android-App ist nicht im Programm"),
    }

    // A working folder and the shell.
    let dir = std::env::temp_dir().join(format!("fadis-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        r.must(false, format!("Arbeitsordner: {e}"));
        return finish(r);
    }
    let ws = match Workspace::open(&dir) {
        Ok(ws) => ws,
        Err(e) => {
            r.must(false, format!("Arbeitsordner öffnen: {e}"));
            return finish(r);
        }
    };
    r.info(format!(
        "Arbeitsordner: {} · Shell: {}",
        crate::platform::plain_path(ws.root()),
        crate::tools::shell_label()
    ));

    let echo = if cfg!(windows) {
        "Write-Output 'hallo äöü €'; cmd /c exit 3"
    } else {
        "echo 'hallo äöü €'; exit 3"
    };
    let out = run(&ws, "run_command", json!({"command": echo}));
    r.must(
        out.text.contains("exit code 3") && out.text.contains("hallo äöü €"),
        format!(
            "Befehl: Ausgabe in UTF-8 und Exit-Code 3 → {}",
            out.text.replace('\n', " | ")
        ),
    );

    let ok = run(&ws, "run_command", json!({"command": "git --version"}));
    r.info(format!("git: {}", ok.text.replace('\n', " | ")));

    let failing = if cfg!(windows) {
        "Get-Item gibt-es-nicht.txt"
    } else {
        "ls gibt-es-nicht.txt"
    };
    let out = run(&ws, "run_command", json!({"command": failing}));
    r.must(
        !out.ok && !out.text.contains("exit code 0"),
        format!(
            "Fehlschlag wird als Fehler gemeldet: {}",
            out.text.replace('\n', " | ")
        ),
    );

    // A command that runs too long is stopped with everything it started.
    let sleeper = if cfg!(windows) {
        "cmd /c ping -n 40 127.0.0.1 > $null; Start-Sleep 40"
    } else {
        "sleep 40"
    };
    let started = Instant::now();
    let out = run(
        &ws,
        "run_command",
        json!({"command": sleeper, "timeout_secs": 2}),
    );
    r.must(
        started.elapsed() < Duration::from_secs(15) && out.text.contains("timeout"),
        format!(
            "Zeitüberschreitung beendet den Befehl nach {:.1} s",
            started.elapsed().as_secs_f32()
        ),
    );

    // Files, with and without the sandbox.
    let out = run(
        &ws,
        "write_file",
        json!({"path": "ordner/äöü datei.txt", "content": "eins\nzwei\n"}),
    );
    r.must(
        out.ok,
        format!("Datei schreiben: {}", first_line(&out.text)),
    );
    if cfg!(windows) {
        let out = run(&ws, "read_file", json!({"path": "ordner\\äöü datei.txt"}));
        r.must(
            out.ok,
            format!("Datei lesen (Rückwärtsstrich): {}", first_line(&out.text)),
        );
    }
    for outside in [
        "..",
        "../..",
        "~/..",
        if cfg!(windows) { r"C:\Windows" } else { "/etc" },
    ] {
        let out = run(&ws, "list_dir", json!({"path": outside}));
        r.must(
            !out.ok && out.text.contains("outside"),
            format!("Außerhalb des Ordners gesperrt: {outside}"),
        );
    }

    // Screen, mouse, clipboard, keyboard: only with a desktop.
    let out = run(&ws, "computer_action", json!({"action": "screen_info"}));
    r.info(format!("Bildschirm: {}", first_line(&out.text)));
    let out = run(&ws, "computer_action", json!({"action": "screenshot"}));
    let shot_ok = out.ok && out.image.is_some();
    r.info(format!(
        "Screenshot: {} {}",
        if shot_ok { "ok" } else { "nicht möglich" },
        first_line(&out.text)
    ));
    let out = run(
        &ws,
        "computer_action",
        json!({"action": "move", "x": 120, "y": 80}),
    );
    r.info(format!("Maus bewegen: {}", first_line(&out.text)));
    let out = run(&ws, "computer_action", json!({"action": "screen_info"}));
    r.info(format!("Mausposition danach: {}", out.text));

    let out = run(
        &ws,
        "browser_action",
        json!({"action": "copy", "text": "Zwischenablage äöü € 😀"}),
    );
    r.info(format!(
        "Zwischenablage schreiben: {}",
        first_line(&out.text)
    ));
    let out = run(&ws, "browser_action", json!({"action": "copy"}));
    r.info(format!(
        "Zwischenablage lesen: {}",
        out.text.replace('\n', " | ")
    ));

    if cfg!(windows) && std::env::var_os("FADIS_SELFTEST_NOTEPAD").is_some() {
        notepad_round_trip(&ws, &mut r);
    }

    let _ = std::fs::remove_dir_all(&dir);
    finish(r)
}

/// Opens Notepad, types, selects all, copies, and reads the clipboard back:
/// focus, keyboard and clipboard in one go.
fn notepad_round_trip(ws: &Workspace, r: &mut Report) {
    let out = run(
        ws,
        "computer_action",
        json!({"action": "open_app", "app": "notepad"}),
    );
    r.info(format!("Notepad öffnen: {}", first_line(&out.text)));
    std::thread::sleep(Duration::from_secs(2));
    let out = run(
        ws,
        "computer_action",
        json!({"action": "type", "text": "Hallo Nemotron äöü"}),
    );
    r.info(format!("Notepad tippen: {}", first_line(&out.text)));
    let out = run(
        ws,
        "computer_action",
        json!({"action": "key", "text": "ctrl+a"}),
    );
    r.info(format!("Notepad Strg+A: {}", first_line(&out.text)));
    let out = run(
        ws,
        "computer_action",
        json!({"action": "key", "text": "ctrl+c"}),
    );
    r.info(format!("Notepad Strg+C: {}", first_line(&out.text)));
    std::thread::sleep(Duration::from_millis(300));
    let out = run(ws, "browser_action", json!({"action": "copy"}));
    let got = out.text.contains("Hallo Nemotron äöü");
    r.info(format!(
        "Notepad: Tippen, Markieren, Kopieren {} → {}",
        if got {
            "funktioniert"
        } else {
            "ergab etwas anderes"
        },
        out.text.replace('\n', " | ")
    ));
    let out = run(ws, "computer_action", json!({"action": "screenshot"}));
    r.info(format!("Screenshot mit Notepad: {}", first_line(&out.text)));
    // Close it for sure, without a question about saving: a Notepad that stays
    // open would keep whoever started the test waiting.
    #[cfg(windows)]
    {
        use std::process::{Command, Stdio};
        let mut kill = Command::new(crate::winsys::system_dir().join("taskkill.exe"));
        crate::platform::quiet(&mut kill)
            .args(["/IM", "notepad.exe", "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = kill.status();
    }
}

fn finish(mut r: Report) -> bool {
    let verdict = if r.ok { "alles in Ordnung" } else { "FEHLER" };
    r.line("====", format!("Selbsttest: {verdict}"));
    if let Some(path) = std::env::var_os("FADIS_SELFTEST_FILE")
        && let Ok(mut file) = std::fs::File::create(path)
    {
        let _ = writeln!(file, "{}", r.lines.join("\n"));
    }
    r.ok
}
