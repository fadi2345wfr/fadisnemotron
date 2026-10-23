//! Windows system calls: opening addresses and programs, screen, mouse,
//! keyboard, clipboard, windows and sleep prevention.
//!
//! Everything goes through documented Win32 functions with plain arguments.
//! No text from the model or from a web page is ever put into a command line
//! or a script: a name only picks something from a list we made ourselves,
//! and text only goes into the keyboard queue or the clipboard.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::Once;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, GlobalFree, HWND, LPARAM, POINT};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::System::Power::{
    ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VkKeyScanW,
};
use windows_sys::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SM_CXSCREEN, SM_CYSCREEN, SW_RESTORE, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow,
};

use crate::computer::{Chord, Key};

/// A zero-terminated UTF-16 copy for the system.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

// ---------------------------------------------------------------------- opening

/// Opens an address, file or program the way a double click does, without a
/// message box if it does not work.
pub fn shell_open(target: &str) -> Result<(), String> {
    let file = wide(target);
    let verb = wide("open");
    // SAFETY: the structure is zeroed and then filled in; both strings are
    // zero-terminated and outlive the call.
    let ok = unsafe {
        let mut info: SHELLEXECUTEINFOW = zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        ShellExecuteExW(&mut info) != 0
    };
    if ok {
        Ok(())
    } else {
        Err(format!("'{target}' konnte nicht geöffnet werden"))
    }
}

// ------------------------------------------------------------------ sleep guard

/// Keeps the PC (and, if asked, the screen) awake. Windows ties this to the
/// calling thread, so it has to be the same thread each time: the window's.
pub fn keep_awake(system: bool, display: bool) {
    let mut flags = ES_CONTINUOUS;
    if system {
        flags |= ES_SYSTEM_REQUIRED;
    }
    if display {
        flags |= ES_DISPLAY_REQUIRED;
    }
    // SAFETY: plain call with flags.
    unsafe {
        SetThreadExecutionState(flags);
    }
}

// ----------------------------------------------------------------------- screen

/// Makes coordinates mean physical pixels, for the screenshot, the mouse and
/// the numbers the model sees. The window toolkit usually did this already.
fn dpi_aware() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: plain call; it fails harmlessly when the setting is already made.
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    });
}

/// The main display in pixels.
pub fn display_size() -> (f64, f64) {
    dpi_aware();
    // SAFETY: plain queries.
    unsafe {
        (
            f64::from(GetSystemMetrics(SM_CXSCREEN)),
            f64::from(GetSystemMetrics(SM_CYSCREEN)),
        )
    }
}

pub fn mouse_position() -> Option<(f64, f64)> {
    dpi_aware();
    let mut at = POINT { x: 0, y: 0 };
    // SAFETY: `at` is valid for the call.
    (unsafe { GetCursorPos(&mut at) } != 0).then(|| (f64::from(at.x), f64::from(at.y)))
}

/// The main display as a picture.
pub fn capture_screen() -> Result<image::RgbImage, String> {
    const LOCKED: &str =
        "Der Bildschirm kann nicht gelesen werden. Ist der PC gesperrt oder der Bildschirm aus?";
    let (width, height) = display_size();
    let (w, h) = (width as i32, height as i32);
    if w <= 0 || h <= 0 {
        return Err("die Bildschirmgröße ist unbekannt".into());
    }
    let mut pixels = vec![0u8; w as usize * h as usize * 4];
    // SAFETY: every handle is checked and released again; the buffer holds
    // exactly w * h 32-bit pixels, as `GetDIBits` is told.
    let copied = unsafe {
        let screen = GetDC(null_mut());
        if screen.is_null() {
            return Err(LOCKED.into());
        }
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let mut lines = 0;
        if !memory.is_null() && !bitmap.is_null() {
            let before = SelectObject(memory, bitmap);
            let ok = BitBlt(memory, 0, 0, w, h, screen, 0, 0, SRCCOPY | CAPTUREBLT) != 0;
            // A bitmap must not be in a device context while its bits are read.
            SelectObject(memory, before);
            if ok {
                let mut info: BITMAPINFO = zeroed();
                info.bmiHeader = BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    // Negative: the top row first.
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..zeroed()
                };
                lines = GetDIBits(
                    screen,
                    bitmap,
                    0,
                    h as u32,
                    pixels.as_mut_ptr().cast::<c_void>(),
                    &mut info,
                    DIB_RGB_COLORS,
                );
            }
        }
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        if !memory.is_null() {
            DeleteDC(memory);
        }
        ReleaseDC(null_mut(), screen);
        lines
    };
    if copied != h {
        return Err(LOCKED.into());
    }
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for bgra in pixels.as_chunks::<4>().0 {
        rgb.extend_from_slice(&[bgra[2], bgra[1], bgra[0]]);
    }
    image::RgbImage::from_raw(w as u32, h as u32, rgb)
        .ok_or_else(|| "das Bild hat nicht die erwartete Größe".to_owned())
}

// ------------------------------------------------------------------------ input

const BLOCKED: &str = "Windows hat die Eingabe nicht angenommen. Läuft das Zielfenster mit Administratorrechten? Dann darf Fadis Nemotron nichts hineintippen oder -klicken, solange es selbst nicht als Administrator läuft.";

fn send(inputs: &[INPUT]) -> Result<(), String> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: the slice is valid, and its length and element size are passed along.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(BLOCKED.to_owned())
    }
}

fn mouse(flags: u32, x: i32, y: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: x,
                dy: y,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key(virtual_key: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// One mouse move to a pixel of the main display, as the mouse itself would send it.
fn move_input(x: f64, y: f64) -> INPUT {
    let (width, height) = display_size();
    // The system wants 0..=65535 over the whole display.
    let scale = |v: f64, extent: f64| {
        (v.clamp(0.0, extent - 1.0) * 65535.0 / (extent - 1.0).max(1.0)).round() as i32
    };
    mouse(
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
        scale(x, width),
        scale(y, height),
        0,
    )
}

/// Moves the pointer in a few steps, like a hand, not in one jump.
pub fn glide_to(x: f64, y: f64) -> Result<(), String> {
    let (from_x, from_y) = mouse_position().unwrap_or((x, y));
    const STEPS: u32 = 8;
    for step in 1..=STEPS {
        let t = f64::from(step) / f64::from(STEPS);
        send(&[move_input(
            from_x + (x - from_x) * t,
            from_y + (y - from_y) * t,
        )])?;
        pause(8);
    }
    Ok(())
}

pub fn click(x: f64, y: f64, right: bool, count: i64) -> Result<(), String> {
    glide_to(x, y)?;
    pause(40);
    let (down, up) = if right {
        (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
    } else {
        (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
    };
    for _ in 0..count {
        send(&[mouse(down, 0, 0, 0)])?;
        pause(35);
        send(&[mouse(up, 0, 0, 0)])?;
        pause(60);
    }
    Ok(())
}

pub fn drag(x: f64, y: f64, to_x: f64, to_y: f64) -> Result<(), String> {
    glide_to(x, y)?;
    pause(40);
    send(&[mouse(MOUSEEVENTF_LEFTDOWN, 0, 0, 0)])?;
    pause(60);
    const STEPS: u32 = 12;
    for step in 1..=STEPS {
        let t = f64::from(step) / f64::from(STEPS);
        send(&[move_input(x + (to_x - x) * t, y + (to_y - y) * t)])?;
        pause(15);
    }
    pause(40);
    send(&[mouse(MOUSEEVENTF_LEFTUP, 0, 0, 0)])
}

/// Positive scrolls down. One line is one notch of the mouse wheel.
pub fn scroll(lines: i32) -> Result<(), String> {
    let notches = lines.clamp(-100, 100);
    send(&[mouse(MOUSEEVENTF_WHEEL, 0, 0, -notches * 120)])
}

/// Writes text into the front window, character by character, as if typed.
/// Any character works, whatever the keyboard layout.
pub fn type_text(text: &str) -> Result<(), String> {
    let mut batch: Vec<INPUT> = Vec::new();
    for ch in text.chars() {
        match ch {
            '\r' => {}
            '\n' => batch.extend(tap(0x0D, false)),
            '\t' => batch.extend(tap(0x09, false)),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units).iter().copied() {
                    batch.push(key(0, unit, KEYEVENTF_UNICODE));
                    batch.push(key(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                }
            }
        }
        if batch.len() >= 64 {
            send(&batch)?;
            batch.clear();
            pause(4);
        }
    }
    send(&batch)
}

/// Down and up of one virtual key.
fn tap(virtual_key: u16, extended: bool) -> [INPUT; 2] {
    let flag = if extended { KEYEVENTF_EXTENDEDKEY } else { 0 };
    [
        key(virtual_key, 0, flag),
        key(virtual_key, 0, flag | KEYEVENTF_KEYUP),
    ]
}

/// Presses a key combination such as ctrl+shift+t.
pub fn press(chord: &Chord) -> Result<(), String> {
    let mut inputs: Vec<INPUT> = Vec::new();
    for m in &chord.modifiers {
        inputs.push(key(m.virtual_key(), 0, 0));
    }
    match &chord.key {
        Key::Virtual { code, extended } => inputs.extend(tap(*code, *extended)),
        Key::Character(c) => {
            let mut units = [0u16; 2];
            let encoded = c.encode_utf16(&mut units);
            if chord.modifiers.is_empty() {
                // A plain character is typed, which works on every layout.
                return type_text(&c.to_string());
            }
            let &[unit] = &*encoded else {
                return Err(format!("'{c}' is not a key"));
            };
            // SAFETY: plain call.
            let scan = unsafe { VkKeyScanW(unit) };
            if scan == -1 {
                return Err(format!("the key '{c}' is not on the keyboard layout"));
            }
            // The high byte says that shift, ctrl or alt are needed as well;
            // the combination the caller asked for already carries its own.
            inputs.extend(tap((scan & 0xFF) as u16, false));
        }
    }
    for m in chord.modifiers.iter().rev() {
        inputs.push(key(m.virtual_key(), 0, KEYEVENTF_KEYUP));
    }
    send(&inputs)
}

// -------------------------------------------------------------------- clipboard

/// The clipboard may be held by another program for a moment.
fn open_clipboard() -> Result<(), String> {
    for _ in 0..20 {
        // SAFETY: plain call; a null owner window is allowed.
        if unsafe { OpenClipboard(null_mut()) } != 0 {
            return Ok(());
        }
        pause(25);
    }
    Err("die Zwischenablage ist gerade von einem anderen Programm belegt".into())
}

/// The text on the clipboard; empty if there is none.
pub fn clipboard_text() -> Result<String, String> {
    open_clipboard()?;
    // SAFETY: the clipboard is open until `CloseClipboard`; the handle is
    // locked while it is read and the text stops at its zero.
    let text = unsafe {
        let mut text = String::new();
        if IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT)) != 0 {
            let handle = GetClipboardData(u32::from(CF_UNICODETEXT));
            if !handle.is_null() {
                let units = GlobalLock(handle).cast::<u16>();
                if !units.is_null() {
                    let mut len = 0;
                    // Bounded, in case a program left an unterminated block.
                    while len < 16 * 1024 * 1024 && *units.add(len) != 0 {
                        len += 1;
                    }
                    text = String::from_utf16_lossy(std::slice::from_raw_parts(units, len));
                    GlobalUnlock(handle);
                }
            }
        }
        CloseClipboard();
        text
    };
    Ok(text)
}

pub fn set_clipboard_text(text: &str) -> Result<(), String> {
    let units = wide(text);
    let bytes = units.len() * size_of::<u16>();
    open_clipboard()?;
    // SAFETY: the memory is allocated for the clipboard, filled while locked,
    // and handed over to the system only if `SetClipboardData` takes it;
    // otherwise it is freed here.
    let stored = unsafe {
        EmptyClipboard();
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes);
        let mut stored = false;
        if !memory.is_null() {
            let target = GlobalLock(memory).cast::<u16>();
            if !target.is_null() {
                std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
                GlobalUnlock(memory);
                stored = !SetClipboardData(u32::from(CF_UNICODETEXT), memory).is_null();
            }
            if !stored {
                GlobalFree(memory);
            }
        }
        CloseClipboard();
        stored
    };
    if stored {
        Ok(())
    } else {
        Err("der Text konnte nicht in die Zwischenablage gelegt werden".into())
    }
}

// ---------------------------------------------------------------------- windows

struct TopWindow {
    handle: HWND,
    title: String,
    /// Program file name without ".exe", lower case.
    program: String,
}

unsafe extern "system" fn collect(handle: HWND, lparam: LPARAM) -> i32 {
    // SAFETY: `lparam` is the vector `top_windows` passed to `EnumWindows`.
    let list = unsafe { &mut *(lparam as *mut Vec<TopWindow>) };
    // SAFETY: plain queries on a window handle the system just gave us.
    unsafe {
        if IsWindowVisible(handle) == 0 {
            return 1;
        }
        let len = GetWindowTextLengthW(handle);
        if len <= 0 {
            return 1;
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let got = GetWindowTextW(handle, buffer.as_mut_ptr(), buffer.len() as i32);
        if got <= 0 {
            return 1;
        }
        let title = String::from_utf16_lossy(&buffer[..got as usize]);
        let mut pid = 0u32;
        GetWindowThreadProcessId(handle, &mut pid);
        list.push(TopWindow {
            handle,
            title,
            program: program_name(pid),
        });
    }
    1
}

/// The program file of a process, without folder and ".exe", lower case.
fn program_name(pid: u32) -> String {
    // SAFETY: the handle is closed again; the buffer length is passed along.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return String::new();
        }
        let mut buffer = vec![0u16; 1024];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut len) != 0;
        CloseHandle(process);
        if !ok {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buffer[..len as usize]);
        Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }
}

fn top_windows() -> Vec<TopWindow> {
    let mut list: Vec<TopWindow> = Vec::new();
    // SAFETY: the callback only runs during this call and gets the vector back.
    unsafe {
        EnumWindows(Some(collect), &mut list as *mut Vec<TopWindow> as LPARAM);
    }
    list
}

fn bring_to_front(handle: HWND) {
    // SAFETY: plain window calls. Windows only lets the program that has the
    // keyboard focus change it, so this joins the input of the front window
    // for a moment.
    unsafe {
        if IsIconic(handle) != 0 {
            ShowWindow(handle, SW_RESTORE);
        }
        let front = GetForegroundWindow();
        let front_thread = if front.is_null() {
            0
        } else {
            GetWindowThreadProcessId(front, null_mut())
        };
        let me = GetCurrentThreadId();
        let joined =
            front_thread != 0 && front_thread != me && AttachThreadInput(me, front_thread, 1) != 0;
        BringWindowToTop(handle);
        SetForegroundWindow(handle);
        if joined {
            AttachThreadInput(me, front_thread, 0);
        }
    }
}

/// Start menu shortcuts (`.lnk`) of all users and of this user.
fn start_menu_shortcuts() -> Vec<(String, PathBuf)> {
    let roots = ["ProgramData", "APPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(|base| PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs"));
    let mut found = Vec::new();
    for root in roots {
        let mut folders = vec![(root, 0)];
        while let Some((folder, depth)) = folders.pop() {
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if depth < 3 {
                        folders.push((path, depth + 1));
                    }
                } else if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
                    && let Some(name) = path.file_stem()
                {
                    found.push((name.to_string_lossy().to_lowercase(), path));
                }
                if found.len() > 5000 {
                    return found;
                }
            }
        }
    }
    found
}

/// Names the system can start directly (`notepad`, `calc`, `msedge`).
fn is_plain_program_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 60
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-' | '+'))
}

/// Brings a program to the front, or starts it. `name` is what a person would
/// say: "notepad", "Google Chrome", "Word". It only chooses among running
/// windows, Start menu shortcuts and program names; it is never run as typed.
pub fn open_app(name: &str) -> Result<(), String> {
    let wanted = name.trim().to_lowercase();
    let program = wanted.strip_suffix(".exe").unwrap_or(&wanted).to_owned();
    if program.is_empty() || program.chars().any(char::is_control) {
        return Err(format!("the app '{name}' was not found"));
    }
    let windows = top_windows();
    if let Some(running) = windows.iter().find(|w| w.program == program) {
        bring_to_front(running.handle);
        return Ok(());
    }
    let shortcuts = start_menu_shortcuts();
    if let Some((_, lnk)) = shortcuts.iter().find(|(stem, _)| *stem == program) {
        return shell_open(&lnk.to_string_lossy());
    }
    if is_plain_program_name(&program) && shell_open(&program).is_ok() {
        return Ok(());
    }
    // The closest shortcut: the shortest name that contains the words.
    if let Some((_, lnk)) = shortcuts
        .iter()
        .filter(|(stem, _)| stem.contains(&program))
        .min_by_key(|(stem, _)| stem.len())
    {
        return shell_open(&lnk.to_string_lossy());
    }
    if let Some(window) = windows
        .iter()
        .find(|w| w.title.to_lowercase().contains(&program))
    {
        bring_to_front(window.handle);
        return Ok(());
    }
    Err(format!("the app '{name}' was not found"))
}

// ---------------------------------------------------------------------- helpers

/// The folder of the Windows system programs, normally `C:\Windows\System32`.
pub fn system_dir() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("System32")
}

/// Ends a process and everything it started.
pub fn kill_tree(pid: u32) {
    let mut cmd = std::process::Command::new(system_dir().join("taskkill.exe"));
    crate::platform::quiet(&mut cmd)
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = cmd.status();
}

/// This computer's name.
pub fn computer_name() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
}
