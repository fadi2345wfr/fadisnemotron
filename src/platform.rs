//! What differs between Windows, macOS and Linux: how the computer is called
//! in texts, how addresses and folders are opened, and how a program is
//! started without a console window.

use std::path::Path;
use std::process::Command;

/// How the user's computer is called in texts shown to them and to the model.
pub fn computer() -> &'static str {
    if cfg!(windows) {
        "PC"
    } else if cfg!(target_os = "macos") {
        "Mac"
    } else {
        "Rechner"
    }
}

/// The key the shortcuts of this system are built on, as written in hints:
/// "⌘" on the Mac, "Strg+" elsewhere.
pub fn command_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Strg+"
    }
}

/// The file manager's name, for buttons.
pub fn file_manager() -> &'static str {
    if cfg!(windows) {
        "Explorer"
    } else if cfg!(target_os = "macos") {
        "Finder"
    } else {
        "Dateimanager"
    }
}

/// Makes a program started from `cmd` run without a console window. The
/// Windows build is a program without a console of its own: without this,
/// every `powershell` or `taskkill` would flash a black window.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Opens a web address in the user's browser. Callers pass addresses that
/// went through `computer::valid_web_url`.
pub fn open_address(url: &str) {
    #[cfg(windows)]
    {
        let _ = crate::winsys::shell_open(url);
    }
    #[cfg(not(windows))]
    {
        use std::process::Stdio;
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = Command::new(opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// Shows a folder in the file manager.
pub fn open_folder(path: &Path) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("explorer.exe");
        cmd.arg(plain_path(path));
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = Command::new(opener).arg(path).spawn();
    }
}

/// Opens the system's power settings (what closing the lid does).
pub fn open_power_settings() {
    #[cfg(windows)]
    {
        let _ = crate::winsys::shell_open("powercfg.cpl");
    }
}

/// A path as the user knows it. Windows reports canonical paths as
/// `\\?\C:\Users\…`, which Explorer, `cmd` and most people do not know.
pub fn plain_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    match without_verbatim_prefix(&text) {
        Some(plain) => plain,
        None => text.into_owned(),
    }
}

/// `\\?\C:\a\b` becomes `C:\a\b` and `\\?\UNC\server\share` becomes
/// `\\server\share`; `None` if there is nothing to change or the short form
/// would mean something else (reserved device names, trailing dots or spaces,
/// too long for programs without long-path support).
fn without_verbatim_prefix(text: &str) -> Option<String> {
    let rest = text.strip_prefix(r"\\?\")?;
    if let Some(share) = rest.strip_prefix(r"UNC\") {
        return Some(format!(r"\\{share}"));
    }
    let bytes = rest.as_bytes();
    let drive =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    if !drive || rest.len() >= 240 {
        return None;
    }
    let odd = rest[3..].split('\\').any(|part| {
        const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let numbered = |prefix: &str| {
            stem.strip_prefix(prefix)
                .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit() && n != "0")
        };
        part.ends_with(['.', ' '])
            || RESERVED.contains(&stem.as_str())
            || numbered("COM")
            || numbered("LPT")
    });
    (!odd).then(|| rest.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_go_away_when_the_short_form_means_the_same() {
        let plain = |s: &str| without_verbatim_prefix(s);
        assert_eq!(
            plain(r"\\?\C:\Users\Fadi\proj").as_deref(),
            Some(r"C:\Users\Fadi\proj")
        );
        assert_eq!(plain(r"\\?\C:\").as_deref(), Some(r"C:\"));
        assert_eq!(
            plain(r"\\?\UNC\server\share\x").as_deref(),
            Some(r"\\server\share\x")
        );
        // Nothing to do, or not safe to do.
        assert_eq!(plain(r"C:\Users"), None);
        assert_eq!(plain("/home/fadi"), None);
        assert_eq!(plain(r"\\?\C:\proj\aux"), None);
        assert_eq!(plain(r"\\?\C:\proj\NUL.txt"), None);
        assert_eq!(plain(r"\\?\C:\proj\com1"), None);
        assert_eq!(plain(r"\\?\C:\proj\trailing."), None);
        assert_eq!(plain(&format!(r"\\?\C:\{}", "a".repeat(260))), None);
        assert_eq!(
            plain(r"\\?\C:\proj\com10").as_deref(),
            Some(r"C:\proj\com10")
        );
        assert_eq!(
            plain(r"\\?\Volume{1234}\x"),
            None,
            "volume paths stay as they are"
        );
    }

    #[test]
    fn plain_path_leaves_ordinary_paths_alone() {
        assert_eq!(plain_path(Path::new("/home/fadi/proj")), "/home/fadi/proj");
    }
}
