mod app;
mod audio;
mod axis;
mod colormap;
mod dsp;
mod music;
mod waterfall;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([720.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Tonescope",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
