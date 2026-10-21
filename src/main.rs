// release 构建在 Windows 上不弹控制台窗口；debug 保留，方便看 panic 输出
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod audio;
mod axis;
mod cli;
mod colormap;
mod dsp;
mod spectrogram;
mod strip;
mod worker;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

// clap 默认把文档注释当成 --help 的说明；结构体上的写给开发者，不进 --help
#[derive(Parser, Debug)]
#[command(about = "Test keyword-spotting ONNX bundles on the microphone or on audio files")]
struct Cli {
    /// Bundles to import at start: folders, .zip files or bundle json files
    #[arg(long = "import", value_name = "BUNDLE")]
    import: Vec<PathBuf>,
    /// A wav or flac file to open instead of listening to the microphone
    #[arg(long, value_name = "AUDIO")]
    open: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the bundles over files without a window, one JSON line per trigger (as `kws stream`)
    Stream(cli::Stream),
}

// 先用 glow（OpenGL 2+）；远程桌面会话、没装显卡驱动的机器上只有 OpenGL 1.1，
// 这时改用 wgpu（Windows 上走 D3D12，没有 GPU 时还有 WARP 软件渲染）。
// release 版没有控制台，不回退的话程序会一声不响地退出。
fn main() -> eframe::Result {
    let app = appkit::app!("Kwscope");
    let cli: Cli = appkit::cli::parse(&app);
    if let Some(Command::Stream(args)) = cli.command {
        if let Err(e) = cli::stream(args) {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let run = |renderer| {
        let (bundles, audio) = (cli.import.clone(), cli.open.clone());
        let options = eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 760.0])
                .with_min_inner_size([760.0, 480.0])
                .with_drag_and_drop(true)
                .with_icon(
                    eframe::icon_data::from_png_bytes(include_bytes!(
                        "../packaging/icon/kwscope.png"
                    ))
                    .expect("packaging/icon/kwscope.png is a valid PNG"),
                ),
            renderer,
            ..Default::default()
        };
        eframe::run_native(
            app.display_name,
            options,
            Box::new(move |cc| Ok(Box::new(app::App::new(cc, app, bundles, audio)))),
        )
    };
    run(eframe::Renderer::Glow).or_else(|e| {
        eprintln!("glow renderer failed ({e}); retrying with wgpu");
        run(eframe::Renderer::Wgpu)
    })
}
