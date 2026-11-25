//! MOSS-Pocket 桌面应用入口。

#![windows_subsystem = "windows"]

mod app;
mod audio;
mod clipboard;
mod editor_ui;
mod export;
mod export_ui;
mod file_picker;
mod find_bar;
mod fonts;
#[cfg(test)]
mod frontend_matrix;
#[cfg(test)]
mod headless;
mod i18n;
mod lock_screen;
#[cfg(test)]
mod matrix;
mod media;
mod media_view;
#[cfg(windows)]
mod native_audio;
mod notes_ui;
mod recording;
#[cfg(test)]
mod scenarios;
mod session;
mod settings;
mod settings_actions;
mod settings_ui;
mod stream;
mod theme;
mod video;
mod win;
mod window_ui;

use eframe::egui;
use std::path::PathBuf;

/// 窗口和任务栏上显示的图标，编译时就嵌在程序里。
fn window_icon() -> Option<egui::IconData> {
    let png = include_bytes!("../assets/relay-icon.png");
    let image = image::load_from_memory(png).ok()?.to_rgba8();
    Some(egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

fn main() {
    // 便携模式：数据目录和设置文件都跟着 exe 走。
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    // 同一份数据同一时间只允许一份程序在写：两份同时写会互相覆盖。
    // 出错提示按用户设置的语言来，所以先读一次设置。
    let tr = crate::i18n::Tr(app::resolve_language(settings::Settings::load(&exe_dir).language));
    let data_dir = mp_core::vault::Vault::data_dir(&exe_dir);
    let _ = std::fs::create_dir_all(&data_dir);
    let Some(_instance_lock) = win::lock_instance(&data_dir.join(".lock")) else {
        win::show_error_box(tr.already_open(), app::APP_TITLE);
        return;
    };

    // 自测模式：设了环境变量 MOSS_POCKET_BENCHMARK 时，窗口不激活、不进任务栏、放到屏幕外，
    // 程序自己记下启动耗时和内存写到指定文件后退出。不抢前台，桌面上也看不到任何东西。
    let benchmark = std::env::var_os("MOSS_POCKET_BENCHMARK").map(PathBuf::from);
    let viewport = egui::ViewportBuilder::default()
        .with_title(app::APP_TITLE)
        .with_app_id("moss.pocket")
        .with_decorations(false);
    let viewport = match &benchmark {
        Some(_) => viewport.with_active(false).with_taskbar(false).with_position([-20000.0, -20000.0]),
        None => viewport,
    };
    let viewport = match window_icon() {
        Some(icon) => viewport.with_icon(std::sync::Arc::new(icon)),
        None => viewport,
    };
    let options = |renderer| eframe::NativeOptions {
        renderer,
        viewport: viewport
            .clone()
            .with_inner_size([1120.0, 740.0])
            .with_min_inner_size([720.0, 480.0])
            .with_transparent(true),
        // Ctrl+C、X、V 自己处理（见 `clipboard.rs`）：窗口层的翻译不认剪贴板里的文件列表。
        clipboard_shortcuts: false,
        ..Default::default()
    };

    // 先用 OpenGL（最轻）。起不来时（远程桌面、显卡驱动异常）退回 DX12。
    let glow_dir = exe_dir.clone();
    let glow_benchmark = benchmark.clone();
    let first = eframe::run_native(
        app::APP_TITLE,
        options(eframe::Renderer::Glow),
        Box::new(move |cc| Ok(Box::new(app::MossPocket::start(cc, glow_dir, glow_benchmark)))),
    );
    if first.is_err() {
        let second = eframe::run_native(
            app::APP_TITLE,
            options(eframe::Renderer::Wgpu),
            Box::new(move |cc| Ok(Box::new(app::MossPocket::start(cc, exe_dir, benchmark)))),
        );
        if let Err(error) = second {
            win::show_error_box(&format!("{}：{error}", tr.window_failed()), app::APP_TITLE);
        }
    }
}
