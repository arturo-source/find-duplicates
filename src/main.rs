#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 760.0]),
        ..Default::default()
    };
    // Optional folder to scan right away: `find-duplicates /path/to/disk`.
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);

    eframe::run_native(
        "Find Duplicates",
        options,
        Box::new(|cc| {
            Ok(Box::new(ui::FindDuplicatesApp::new(
                cc.egui_ctx.clone(),
                initial,
            )))
        }),
    )
}
