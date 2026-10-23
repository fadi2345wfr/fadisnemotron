//! Dictation: the microphone button next to the input field starts the speech
//! recognition of the system itself. The words appear in the input field as
//! if they were typed. This program records nothing and sends nothing; it
//! only presses the button the system provides:
//!
//! - Windows: voice typing (Win+H). The first time, Windows asks whether
//!   "online speech recognition" may be switched on.
//! - macOS: Dictation (the "Start Dictation" command of the Edit menu). The
//!   first time, macOS asks whether dictation may be switched on.

/// Whether this system has a dictation that can be started from here.
pub fn available() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

/// The tooltip of the microphone button.
pub fn hint() -> &'static str {
    if cfg!(windows) {
        "Diktieren: öffnet das Windows-Diktat (Win+H). Beim ersten Mal fragt Windows, ob die Online-Spracherkennung eingeschaltet werden soll."
    } else {
        "Diktieren: startet das Mac-Diktat (sonst die Fn-Taste zweimal drücken). Beim ersten Mal fragt macOS, ob Diktieren eingeschaltet werden soll."
    }
}

/// Starts the dictation; what is said goes to the input field that has the
/// focus. Call it from the thread of the window (the Mac insists on that).
pub fn start() -> Result<(), String> {
    #[cfg(windows)]
    {
        windows()
    }
    #[cfg(target_os = "macos")]
    {
        mac()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err("Diktieren gibt es auf diesem System nicht.".to_owned())
    }
}

#[cfg(windows)]
fn windows() -> Result<(), String> {
    use crate::computer::{Chord, Key, Modifier};
    let chord = Chord {
        modifiers: vec![Modifier::Win],
        key: Key::Character('h'),
    };
    // The input field has to have the focus before voice typing starts, and
    // that takes the window a moment: press the keys from another thread.
    std::thread::Builder::new()
        .name("diktat".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let _ = crate::winsys::press(&chord);
        })
        .map(|_| ())
        .map_err(|e| format!("Das Diktat ließ sich nicht starten: {e}"))
}

#[cfg(target_os = "macos")]
fn mac() -> Result<(), String> {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{MainThreadMarker, class, msg_send, sel};
    const FALLBACK: &str = "Das Mac-Diktat ließ sich nicht starten. Klick ins Eingabefeld und drück zweimal die Fn-Taste (oder die Mikrofon-Taste).";
    if MainThreadMarker::new().is_none() {
        return Err(FALLBACK.to_owned());
    }
    // SAFETY: `sendAction:to:from:` with a nil target sends the action along the
    // responder chain, as the menu item "Start Dictation" does; we are on the
    // main thread, as AppKit wants.
    let handled: Bool = unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        msg_send![app, sendAction: sel!(startDictation:), to: None::<&AnyObject>, from: None::<&AnyObject>]
    };
    if handled.as_bool() {
        Ok(())
    } else {
        Err(FALLBACK.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_only_exists_where_the_system_has_a_dictation() {
        assert_eq!(available(), cfg!(any(windows, target_os = "macos")));
        assert!(hint().starts_with("Diktieren:"));
        #[cfg(not(any(windows, target_os = "macos")))]
        assert!(start().is_err());
    }
}
