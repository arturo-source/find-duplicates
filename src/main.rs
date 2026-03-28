use find_duplicates::{get_duplicated_files, get_shared_parents, list_files};
use std::io;
use std::path::PathBuf;

fn run(path: PathBuf) -> io::Result<String> {
    let paths = list_files(path)?;
    let duplicated_files = get_duplicated_files(paths)?;
    let shared_parents = get_shared_parents(duplicated_files);

    let mut shared_parents_vec: Vec<_> = shared_parents.into_iter().collect();
    shared_parents_vec.sort_by(|a, b| b.1 .0.len().cmp(&a.1 .0.len()));

    let mut output = String::new();
    for ((parent1, parent2), (files1, files2)) in shared_parents_vec {
        output.push_str(&format!("In {parent1:?}:\n"));
        for f in files1 {
            output.push_str(&format!("  {:?}\n", f.file_name().unwrap()));
        }

        output.push_str(&format!("In {parent2:?}:\n"));
        for f in files2 {
            output.push_str(&format!("  {:?}\n", f.file_name().unwrap()));
        }

        output.push('\n');
    }

    Ok(output)
}

struct FindDuplicatesApp {
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
                match run(path) {
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

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([600.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Find Duplicates",
        options,
        Box::new(|_cc| Ok(Box::new(FindDuplicatesApp::default()))),
    )
}
