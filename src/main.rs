// release 构建在 Windows 上不弹控制台窗口；debug 保留，方便看 panic 输出
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod audio;
mod axis;
mod colormap;
mod dsp;
mod music;
mod perf;
mod pitch;
mod waterfall;

// 先用 glow（OpenGL 2+）；远程桌面会话、没装显卡驱动的机器上只有 OpenGL 1.1，
// 这时改用 wgpu（Windows 上走 D3D12，没有 GPU 时还有 WARP 软件渲染）。
// release 版没有控制台，不回退的话程序会一声不响地退出。
fn main() -> eframe::Result {
    let app = appkit::app!("Tonescope");
    let appkit::cli::NoArgs {} = appkit::cli::parse(&app);
    let run = |renderer| {
        let options = eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 760.0])
                .with_min_inner_size([720.0, 420.0])
                .with_icon(
                    eframe::icon_data::from_png_bytes(include_bytes!(
                        "../packaging/icon/tonescope.png"
                    ))
                    .expect("packaging/icon/tonescope.png is a valid PNG"),
                ),
            renderer,
            ..Default::default()
        };
        eframe::run_native(
            app.display_name,
            options,
            Box::new(move |cc| Ok(Box::new(app::App::new(cc, app)))),
        )
    };
    run(eframe::Renderer::Glow).or_else(|e| {
        eprintln!("glow renderer failed ({e}); retrying with wgpu");
        run(eframe::Renderer::Wgpu)
    })
}
