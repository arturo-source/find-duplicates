#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([600.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Find Duplicates",
        options,
        Box::new(|cc| Ok(Box::new(ui::FindDuplicatesApp::new(cc.egui_ctx.clone())))),
    )
}
