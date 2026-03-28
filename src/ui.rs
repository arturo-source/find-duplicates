use find_duplicates::format_duplicates;
use std::path::PathBuf;

pub struct FindDuplicatesApp {
    result: String,
    status: String,
}

impl Default for FindDuplicatesApp {
    fn default() -> Self {
        Self {
            result: String::new(),
            status: "Select a folder to scan for duplicates".into(),
        }
    }
}

impl eframe::App for FindDuplicatesApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("Find Duplicates");
        ui.separator();

        if ui.button("Select Folder").clicked() {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                self.scan(path);
            }
        }

        ui.label(&self.status);
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                ui.label(&self.result);
            });
    }
}

impl FindDuplicatesApp {
    fn scan(&mut self, path: PathBuf) {
        match format_duplicates(path) {
            Ok(output) => {
                if output.is_empty() {
                    self.status = "No duplicates found.".into();
                } else {
                    self.status = "Done.".into();
                }
                self.result = output;
            }
            Err(e) => {
                self.status = format!("Error: {e}");
            }
        }
    }
}
