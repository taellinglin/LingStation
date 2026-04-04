use std::backtrace::Backtrace;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crate::{DawApp, RenderFormat};
use eframe::egui;

pub fn main() -> eframe::Result<()> {
    env_logger::init();
    install_crash_logger();
    install_runtime_working_directory();
    init_windows_com();

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "render" {
        if let Err(err) = run_cli_render(&args[2..]) {
            log::error!("Render failed: {err}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "LingStation",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            configure_fonts(&cc.egui_ctx);
            Box::new(DawApp::default())
        }),
    )
}

fn run_cli_render(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err(
            "Usage: LingStation render <project_folder> <output_folder> [wav|ogg|flac]".to_string(),
        );
    }
    let project_folder = PathBuf::from(args[0].trim());
    let output_folder = PathBuf::from(args[1].trim());
    let format = args.get(2).map(|s| s.as_str()).unwrap_or("wav");
    let render_format = match format.to_ascii_lowercase().as_str() {
        "wav" => RenderFormat::Wav,
        "ogg" => RenderFormat::Ogg,
        "flac" => RenderFormat::Flac,
        other => return Err(format!("Unknown format: {other}")),
    };

    let mut app = DawApp::default();
    app.load_project_from_folder(&project_folder)?;
    app.render_format = render_format;
    app.render_range_start = 0.0;
    app.render_range_end = app.project_end_beats().max(0.25);
    app.render_with_options(&output_folder)?;
    wait_for_render_job(&mut app)
}

fn init_windows_com() {
    #[cfg(windows)]
    {
        engine::hosts::vst3::init_windows_com_for_thread();
    }
}

fn install_runtime_working_directory() {
    if let Ok(cwd) = std::env::current_dir() {
        log::info!("runtime cwd(before): {}", cwd.display());
    }
    let Some(root) = detect_runtime_root() else {
        log::warn!("runtime root: (not found)");
        return;
    };
    log::info!("runtime root(selected): {}", root.display());
    let _ = std::env::set_current_dir(&root);
    if let Ok(cwd) = std::env::current_dir() {
        log::info!("runtime cwd(after): {}", cwd.display());
    }
}

fn detect_runtime_root() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let mut cursor = Some(parent.to_path_buf());
            for _ in 0..5 {
                let Some(dir) = cursor.clone() else {
                    break;
                };
                candidates.push(dir.clone());
                cursor = dir.parent().map(|p| p.to_path_buf());
            }
        }
    }
    candidates
        .into_iter()
        .find(|dir| dir.join("synths").exists() || dir.join("assets").join("sample_kits").exists())
}

fn load_app_icon() -> Option<egui::IconData> {
    None
}

fn configure_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    let mut fonts = FontDefinitions::default();
    // フォントファイルを読み込む
    if let Ok(font_bytes) = std::fs::read("./assets/font.ttf") {
        fonts
            .font_data
            .insert("custom_font".to_owned(), FontData::from_owned(font_bytes));
        // プロポーショナル/モノスペース両方に割り当て
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "custom_font".to_owned());
        fonts
            .families
            .get_mut(&FontFamily::Monospace)
            .unwrap()
            .insert(0, "custom_font".to_owned());
        ctx.set_fonts(fonts);
    }
}

fn wait_for_render_job(app: &mut DawApp) -> Result<(), String> {
    loop {
        let Some(job) = app.render_job.as_ref() else {
            return Ok(());
        };
        if job.finished.load(Ordering::Relaxed) {
            let result = job
                .result
                .lock()
                .ok()
                .and_then(|mut guard| guard.take())
                .unwrap_or_else(|| Ok("Render complete".to_string()));
            app.render_job = None;
            return result.map(|_| ());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[cfg(windows)]
fn install_windows_crash_handler() {}

#[cfg(not(windows))]
fn install_windows_crash_handler() {}

fn install_crash_logger() {
    install_windows_crash_handler();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("crash.log")
        {
            let _ = writeln!(file, "---- crash ----");
            let _ = writeln!(file, "{info}");
            let bt = Backtrace::force_capture();
            let _ = writeln!(file, "{bt:?}");
            log::error!("CRASH: {info}\n{bt:?}");
        }
        default_hook(info);
    }));
}
