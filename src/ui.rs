use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use find_duplicates::{
    DirectoryNode, build_directory_tree, get_duplicated_files, group_files_by_size,
    list_files_with_ignore,
};

const DEFAULT_IGNORE: &[&str] = &[".git", "node_modules", "__pycache__", ".DS_Store"];

fn parse_size(input: &str) -> Option<u64> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    // Try parsing as a plain number (bytes)
    if let Ok(bytes) = input.parse::<u64>() {
        return Some(bytes);
    }

    let (num_str, suffix) = input.split_at(input.find(|c: char| c.is_alphabetic())?);
    let num: f64 = num_str.trim().parse().ok()?;
    let suffix = suffix.trim().to_lowercase();

    let multiplier: f64 = match suffix.as_str() {
        "b" | "byte" | "bytes" => 1.0,
        "kb" | "kilobyte" | "kilobytes" => 1024.0,
        "mb" | "megabyte" | "megabytes" => 1024.0 * 1024.0,
        "gb" | "gigabyte" | "gigabytes" => 1024.0 * 1024.0 * 1024.0,
        "tb" | "terabyte" | "terabytes" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };

    let result = num * multiplier;
    if result.is_finite() && result >= 0.0 {
        Some(result as u64)
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ScanStep {
    Listing,
    Finding,
    Building,
}

struct ScanProgress {
    step: ScanStep,
    progress: f32,
    done: bool,
}

enum ScanMessage {
    Step(ScanStep),
    Progress(f32),
    Done(Result<(DirectoryNode, usize), String>),
}

pub struct FindDuplicatesApp {
    tree: Option<DirectoryNode>,
    root: PathBuf,
    patterns: Vec<String>,
    new_pattern: String,
    scan_rx: Option<mpsc::Receiver<ScanMessage>>,
    scan_progress: Option<ScanProgress>,
    ctx: egui::Context,
    quick_scan: bool,
    min_size: u64,
    size_input: String,
    hovered_files: RefCell<HashSet<PathBuf>>,
}

impl FindDuplicatesApp {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            tree: None,
            root: PathBuf::new(),
            patterns: DEFAULT_IGNORE.iter().map(|s| s.to_string()).collect(),
            new_pattern: String::new(),
            scan_rx: None,
            scan_progress: None,
            ctx,
            quick_scan: true,
            min_size: 64,
            size_input: "64 bytes".into(),
            hovered_files: RefCell::new(HashSet::new()),
        }
    }
}

impl eframe::App for FindDuplicatesApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(ref rx) = self.scan_rx {
            let messages: Vec<_> = rx.try_iter().collect();
            for msg in messages {
                match msg {
                    ScanMessage::Step(step) => {
                        self.scan_progress = Some(ScanProgress {
                            step,
                            progress: 0.0,
                            done: false,
                        });
                    }
                    ScanMessage::Progress(p) => {
                        if let Some(ref mut sp) = self.scan_progress {
                            sp.progress = p;
                        }
                    }
                    ScanMessage::Done(result) => {
                        self.scan_rx = None;
                        if let Some(ref mut sp) = self.scan_progress {
                            sp.progress = 1.0;
                            sp.done = true;
                        }
                        if let Ok((tree, _)) = result {
                            self.tree = Some(tree);
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
            ui.horizontal(|ui| {
                ui.label("Min file size:");
                let response = ui.text_edit_singleline(&mut self.size_input);
                if response.changed() {
                    if let Some(bytes) = parse_size(&self.size_input) {
                        self.min_size = bytes;
                    }
                }
                if let Some(bytes) = parse_size(&self.size_input) {
                    ui.label(format!("({} bytes)", bytes));
                } else if !self.size_input.is_empty() {
                    ui.colored_label(egui::Color32::YELLOW, "e.g. 1MB, 50KB, 64 bytes");
                }
            });
            if ui.button("Select Folder").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.scan(path, self.quick_scan, self.min_size);
                }
            }
        });

        if let Some(ref sp) = self.scan_progress {
            let steps = [
                (ScanStep::Listing, "Listing files"),
                (ScanStep::Finding, "Finding duplicates"),
                (ScanStep::Building, "Building tree"),
            ];
            for (step, label) in steps {
                let done = sp.done || sp.step as usize > step as usize;
                let active = !sp.done && sp.step == step;
                ui.horizontal(|ui| {
                    if done {
                        ui.colored_label(egui::Color32::from_rgb(80, 200, 80), "\u{2713}");
                    } else if active {
                        ui.spinner();
                    } else {
                        ui.colored_label(egui::Color32::GRAY, "\u{25CB}");
                    }
                    let text = if active {
                        egui::RichText::new(label).strong()
                    } else if done {
                        egui::RichText::new(label).color(egui::Color32::from_rgb(80, 200, 80))
                    } else {
                        egui::RichText::new(label).color(egui::Color32::GRAY)
                    };
                    ui.label(text);
                });
                if active {
                    ui.add(
                        egui::ProgressBar::new(sp.progress)
                            .show_percentage()
                            .animate(true),
                    );
                }
            }
        }
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(ref tree) = self.tree {
                    show_node(ui, tree, &self.root, &self.hovered_files);
                }
            });
    }
}

fn show_node(
    ui: &mut egui::Ui,
    node: &DirectoryNode,
    root: &Path,
    hovered_files: &RefCell<HashSet<PathBuf>>,
) {
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
            ui.horizontal(|ui| {
                let show_label = |ui: &mut egui::Ui, text: &str, open_path: &Path, id: &Path| {
                    let was_hovered = hovered_files.borrow().contains(id);
                    let mut rich = egui::RichText::new(text);
                    if was_hovered {
                        rich = rich.underline();
                    }
                    let resp = ui.add(egui::Label::new(rich).sense(egui::Sense::click()));
                    let clicked = resp.clicked();
                    if resp.hovered() {
                        hovered_files.borrow_mut().insert(id.to_path_buf());
                        resp.on_hover_text("click to open the folder")
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                    } else {
                        hovered_files.borrow_mut().remove(id);
                    }
                    if clicked {
                        let _ = open::that(open_path);
                    }
                };

                show_label(
                    ui,
                    &file_name.to_string(),
                    file.parent().unwrap_or(file),
                    file,
                );

                if !others.is_empty() {
                    ui.label("=");
                    for other in others {
                        let rel = other
                            .strip_prefix(root)
                            .unwrap_or(other)
                            .to_string_lossy()
                            .to_string();
                        show_label(ui, &rel, other.parent().unwrap_or(other), other);
                    }
                }
            });
        }
        for child in &node.children {
            show_node(ui, child, root, hovered_files);
        }
    });
}

impl FindDuplicatesApp {
    fn scan(&mut self, path: PathBuf, quick_scan: bool, min_size: u64) {
        self.tree = None;
        self.root = path.clone();
        self.scan_progress = None;

        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);

        let ignore_set: HashSet<String> = self.patterns.iter().cloned().collect();
        let ctx = self.ctx.clone();

        thread::spawn(move || {
            let _ = tx.send(ScanMessage::Step(ScanStep::Listing));
            ctx.request_repaint();

            match list_files_with_ignore(path.clone(), &ignore_set, min_size) {
                Ok(paths) => {
                    let _ = tx.send(ScanMessage::Step(ScanStep::Finding));
                    ctx.request_repaint();

                    let files_by_size = group_files_by_size(&paths);

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
                    let _ = tx.send(ScanMessage::Step(ScanStep::Building));
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
