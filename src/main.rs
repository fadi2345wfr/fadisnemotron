// No console window on Windows; on macOS the app is started from the .app bundle.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod api;
mod app;
mod attachments;
mod chats;
mod config;
mod remote;
mod tools;
mod widgets;

use eframe::egui;

const APP_NAME: &str = "Fadis Nemotron";

fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some(tools::PRINT_ENV_ARG) {
        tools::print_environment();
        return Ok(());
    }
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
        renderer: renderer_from_env(),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(app::NemotronApp::new(cc)))),
    )
}

/// wgpu (Metal on macOS) is the default. `FADIS_RENDERER=glow` switches to
/// OpenGL, e.g. for old Macs or virtual machines without Metal.
fn renderer_from_env() -> eframe::Renderer {
    match std::env::var("FADIS_RENDERER").as_deref() {
        Ok("glow" | "opengl" | "gl") => eframe::Renderer::Glow,
        _ => eframe::Renderer::Wgpu,
    }
}
