use find_duplicates::{
    build_directory_tree, get_duplicated_files, list_files_with_ignore, DirectoryNode,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

const DEFAULT_IGNORE: &[&str] = &[".git", "node_modules", "__pycache__", ".DS_Store"];

enum ScanMessage {
    Status(String),
    Progress(f32),
    Done(Result<(DirectoryNode, usize), String>),
}

pub struct FindDuplicatesApp {
    tree: Option<DirectoryNode>,
    root: PathBuf,
    status: String,
    patterns: Vec<String>,
    new_pattern: String,
    scan_rx: Option<mpsc::Receiver<ScanMessage>>,
    progress: Option<f32>,
    ctx: egui::Context,
    quick_scan: bool,
}

impl FindDuplicatesApp {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            tree: None,
            root: PathBuf::new(),
            status: "Select a folder to scan for duplicates".into(),
            patterns: DEFAULT_IGNORE.iter().map(|s| s.to_string()).collect(),
            new_pattern: String::new(),
            scan_rx: None,
            progress: None,
            ctx,
            quick_scan: true,
        }
    }
}

impl eframe::App for FindDuplicatesApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(ref rx) = self.scan_rx {
            let messages: Vec<_> = rx.try_iter().collect();
            for msg in messages {
                match msg {
                    ScanMessage::Status(s) => self.status = s,
                    ScanMessage::Progress(p) => self.progress = Some(p),
                    ScanMessage::Done(result) => {
                        self.scan_rx = None;
                        self.progress = None;
                        match result {
                            Ok((tree, count)) => {
                                if count == 0 {
                                    self.status = "No duplicates found.".into();
                                } else {
                                    self.status = format!("Found {count} duplicates.");
                                }
                                self.tree = Some(tree);
                            }
                            Err(e) => self.status = format!("Error: {e}"),
                        }
                    }
                }
            }
        }
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

        let scanning = self.scan_rx.is_some();
        ui.add_enabled_ui(!scanning, |ui| {
            ui.checkbox(&mut self.quick_scan, "Quick scan (first 4KB only)");
            if ui.button("Select Folder").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.scan(path, self.quick_scan);
                }
            }
        });

        ui.label(&self.status);
        if let Some(p) = self.progress {
            ui.add(egui::ProgressBar::new(p).show_percentage());
        }
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

fn show_node(ui: &mut egui::Ui, node: &DirectoryNode, root: &Path) {
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
    fn scan(&mut self, path: PathBuf, quick_scan: bool) {
        self.tree = None;
        self.root = path.clone();
        self.status = "Scanning...".into();

        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);

        let ignore_set: HashSet<String> = self.patterns.iter().cloned().collect();
        let ctx = self.ctx.clone();

        thread::spawn(move || {
            let _ = tx.send(ScanMessage::Status("Listing files...".into()));
            ctx.request_repaint();

            match list_files_with_ignore(path.clone(), &ignore_set) {
                Ok(paths) => {
                    let file_count = paths.len();
                    let _ = tx.send(ScanMessage::Status(format!(
                        "Found {file_count} files. Searching for duplicates..."
                    )));
                    ctx.request_repaint();

                    let mut files_by_size: std::collections::HashMap<u64, Vec<PathBuf>> =
                        std::collections::HashMap::new();
                    for p in &paths {
                        if let Ok(m) = p.metadata() {
                            files_by_size.entry(m.len()).or_default().push(p.clone());
                        }
                    }

                    let total_to_compare: usize = files_by_size
                        .values()
                        .filter(|v| v.len() > 1)
                        .map(|v| v.len())
                        .sum();
                    let mut compared = 0usize;

                    let duplicated_files = get_duplicated_files(files_by_size, quick_scan, || {
                        compared += 1;
                        if total_to_compare > 0 {
                            let _ = tx.send(ScanMessage::Progress(
                                compared as f32 / total_to_compare as f32,
                            ));
                            ctx.request_repaint();
                        }
                    });
                    let _ = tx.send(ScanMessage::Status("Building tree...".into()));
                    ctx.request_repaint();

                    let tree = build_directory_tree(&path, duplicated_files);
                    let count = tree.total_count();
                    let _ = tx.send(ScanMessage::Done(Ok((tree, count))));
                    ctx.request_repaint();
                }
                Err(e) => {
                    let _ = tx.send(ScanMessage::Done(Err(e.to_string())));
                    ctx.request_repaint();
                }
            }
        });
    }
}
