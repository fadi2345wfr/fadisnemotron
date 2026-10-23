// No console window on Windows; on macOS the app is started from the .app bundle.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod api;
mod app;
mod attachments;
mod awake;
mod chats;
mod computer;
mod config;
mod dictation;
mod history;
mod markers;
mod platform;
mod powershell;
mod remote;
mod selftest;
mod tools;
mod widgets;
#[cfg(windows)]
mod winsys;

use eframe::egui;

const APP_NAME: &str = "Fadis Nemotron";

fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some(tools::PRINT_ENV_ARG) {
        tools::print_environment();
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some(selftest::ARG) {
        std::process::exit(if selftest::run_all() { 0 } else { 1 });
    }
    let renderer = renderer_from_env();
    match run_window(renderer) {
        // No usable graphics adapter (a virtual machine, an old driver):
        // OpenGL often still works.
        Err(eframe::Error::Wgpu(e)) if std::env::var_os("FADIS_RENDERER").is_none() => {
            eprintln!("wgpu geht nicht ({e}); versuche OpenGL");
            run_window(eframe::Renderer::Glow)
        }
        other => other,
    }
}

fn run_window(renderer: eframe::Renderer) -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
        .expect("Icon ist ein gültiges PNG");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id("de.fadi.FadisNemotron")
            .with_inner_size([1040.0, 780.0])
            .with_min_inner_size([560.0, 420.0])
            .with_icon(icon)
            .with_drag_and_drop(true),
        renderer,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(app::NemotronApp::new(cc)))),
    )
}

/// wgpu (Metal on macOS, Direct3D on Windows) is the default.
/// `FADIS_RENDERER=glow` switches to OpenGL, e.g. for old Macs or virtual
/// machines without a graphics adapter; the program also falls back to it
/// when wgpu finds no adapter.
fn renderer_from_env() -> eframe::Renderer {
    match std::env::var("FADIS_RENDERER").as_deref() {
        Ok("glow" | "opengl" | "gl") => eframe::Renderer::Glow,
        _ => eframe::Renderer::Wgpu,
    }
}
