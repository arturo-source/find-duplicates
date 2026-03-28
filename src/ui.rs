use find_duplicates::{
    build_directory_tree, get_duplicated_files, list_files_with_ignore, DirectoryNode,
};
use std::path::PathBuf;

const DEFAULT_IGNORE: &[&str] = &[".git", "node_modules", "__pycache__", ".DS_Store"];

pub struct FindDuplicatesApp {
    tree: Option<DirectoryNode>,
    root: PathBuf,
    status: String,
    patterns: Vec<String>,
    new_pattern: String,
}

impl Default for FindDuplicatesApp {
    fn default() -> Self {
        Self {
            tree: None,
            root: PathBuf::new(),
            status: "Select a folder to scan for duplicates".into(),
            patterns: DEFAULT_IGNORE.iter().map(|s| s.to_string()).collect(),
            new_pattern: String::new(),
        }
    }
}

impl eframe::App for FindDuplicatesApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("Find Duplicates");
        ui.separator();

        ui.collapsing("Ignore Patterns", |ui| {
            let mut remove_idx = None;
            for (i, pattern) in self.patterns.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(pattern);
                    if ui.small_button("x").clicked() {
                        remove_idx = Some(i);
                    }
                });
            }
            if let Some(i) = remove_idx {
                self.patterns.remove(i);
            }

            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.new_pattern);
                if ui.button("Add").clicked() && !self.new_pattern.is_empty() {
                    self.patterns.push(self.new_pattern.clone());
                    self.new_pattern.clear();
                }
            });
        });

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
        match list_files_with_ignore(path.clone(), &self.patterns) {
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
