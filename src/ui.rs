use find_duplicates::{build_directory_tree, get_duplicated_files, list_files, DirectoryNode};
use std::path::PathBuf;

pub struct FindDuplicatesApp {
    tree: Option<DirectoryNode>,
    root: PathBuf,
    status: String,
}

impl Default for FindDuplicatesApp {
    fn default() -> Self {
        Self {
            tree: None,
            root: PathBuf::new(),
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
                if let Some(ref tree) = self.tree {
                    show_node(ui, tree, &self.root);
                }
            });
    }
}

fn show_node(ui: &mut egui::Ui, node: &DirectoryNode, root: &PathBuf) {
    let name = node
        .path
        .file_name()
        .unwrap_or(node.path.as_os_str())
        .to_string_lossy();
    let count = node.total_count();

    ui.collapsing(format!("{name} ({count})"), |ui| {
        for (file, others) in &node.files {
            let file_name = file
                .file_name()
                .unwrap_or(file.as_os_str())
                .to_string_lossy();
            if others.is_empty() {
                ui.label(file_name.to_string());
            } else {
                let others_text: Vec<String> = others
                    .iter()
                    .map(|p| {
                        p.strip_prefix(root)
                            .unwrap_or(p)
                            .to_string_lossy()
                            .to_string()
                    })
                    .collect();
                ui.label(format!("{file_name} = {}", others_text.join(", ")));
            }
        }
        for child in &node.children {
            show_node(ui, child, root);
        }
    });
}

impl FindDuplicatesApp {
    fn scan(&mut self, path: PathBuf) {
        self.tree = None;
        self.root = path.clone();
        match list_files(path.clone()) {
            Ok(paths) => match get_duplicated_files(paths) {
                Ok(duplicated_files) => {
                    let tree = build_directory_tree(&path, duplicated_files);
                    if tree.total_count() == 0 {
                        self.status = "No duplicates found.".into();
                    } else {
                        self.status = format!("Found {} duplicates.", tree.total_count());
                    }
                    self.tree = Some(tree);
                }
                Err(e) => self.status = format!("Error: {e}"),
            },
            Err(e) => self.status = format!("Error: {e}"),
        }
    }
}
