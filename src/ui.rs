use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use find_duplicates::{
    build_directory_tree, get_duplicated_files, group_files_by_size, list_files_with_ignore,
    DirectoryNode,
};

const DEFAULT_IGNORE: &[&str] = &[".git", "node_modules", "__pycache__", ".DS_Store"];

fn parse_size(input: &str) -> Option<u64> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

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
    expanded_folders: RefCell<HashSet<PathBuf>>,
    hovered_folder: RefCell<Option<PathBuf>>,
    hovered_folder_pos: RefCell<egui::Pos2>,
    folder_info_cache: RefCell<HashMap<PathBuf, Vec<(PathBuf, usize)>> >,
    show_settings: bool,
    settings_pos: egui::Pos2,
}

impl FindDuplicatesApp {
    pub fn new(ctx: egui::Context) -> Self {
        match dark_light::detect() {
            Ok(dark_light::Mode::Dark) => ctx.set_theme(egui::Theme::Dark),
            _ => ctx.set_theme(egui::Theme::Light),
        }
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
            expanded_folders: RefCell::new(HashSet::new()),
            hovered_folder: RefCell::new(None),
            hovered_folder_pos: RefCell::new(egui::Pos2::ZERO),
            folder_info_cache: RefCell::new(HashMap::new()),
            show_settings: false,
            settings_pos: egui::Pos2::ZERO,
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
                    ScanMessage::Done(Ok((tree, _))) => {
                        self.scan_rx = None;
                        if let Some(ref mut sp) = self.scan_progress {
                            sp.progress = 1.0;
                            sp.done = true;
                        }
                        let tree_opt = tree.clone();
                        self.tree = Some(tree);
                        self.populate_folder_cache(&tree_opt);
                    }
                    ScanMessage::Done(Err(_)) => {
                        self.scan_rx = None;
                    }
                }
            }
        }

        let scanning = self.scan_rx.is_some();

        let rect = ui.max_rect();
        let btn_width = 120.0;
        let total_width = btn_width * 2.0 + 16.0;
        let start_x = rect.center().x - total_width / 2.0;
        let btn_rect = egui::Rect::from_min_size(egui::Pos2::new(start_x, 0.0), egui::vec2(total_width, 40.0));

        ui.scope_builder(egui::UiBuilder::new().max_rect(btn_rect), |ui| {
            ui.set_min_size(egui::vec2(total_width, 40.0));
            ui.horizontal(|ui| {
                let btn = ui.button("Select Folder");
                if btn.clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.scan(path, self.quick_scan, self.min_size);
                    }
                }

                ui.add_space(16.0);

                let btn = ui.button("Settings");
                if btn.clicked() {
                    self.settings_pos = btn.rect.center();
                    self.show_settings = !self.show_settings;
                }
            });
        });

        if self.show_settings {
            let rect = ui.max_rect();
            let padding = 40.0;
            let panel_rect = egui::Rect::from_min_size(
                egui::Pos2::new(padding, padding),
                egui::vec2(rect.width() - padding * 2.0, rect.height() - padding * 2.0),
            );

            egui::Area::new("settings_panel".into())
                .current_pos(egui::Pos2::new(padding, padding))
                .movable(false)
                .interactable(true)
                .show(ui.ctx(), |ui: &mut egui::Ui| {
                    egui::Frame::popup(ui.style_mut())
                        .inner_margin(egui::vec2(12.0, 12.0))
                        .show(ui, |ui| {
                            ui.set_min_size(panel_rect.size());

                            ui.heading("Settings");

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

                        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                            let btn = ui.button("Close Settings");
                            if btn.clicked() {
                                self.show_settings = false;
                            }
                        });
                    });
            });
        }

        if scanning {
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
                            ui.colored_label(egui::Color32::from_rgb(80, 200, 80), "✓");
                        } else if active {
                            ui.spinner();
                        } else {
                            ui.colored_label(egui::Color32::GRAY, "○");
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
                    if active && step == ScanStep::Finding {
                        ui.add(egui::ProgressBar::new(sp.progress).show_percentage());
                    }
                }
            }
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(ref tree) = self.tree {
                    show_flat_list(
                        ui,
                        tree,
                        &self.root,
                        &self.hovered_files,
                        &self.expanded_folders,
                        &self.hovered_folder,
                        &self.hovered_folder_pos,
                    );
                }
            });

if let Some(ref hovered) = *self.hovered_folder.borrow() {
            let pos = *self.hovered_folder_pos.borrow();
            let root = &self.root;

            let folder_info = self.folder_info_cache.borrow().get(hovered).cloned();

            if let Some(folder_info) = folder_info {
                if !folder_info.is_empty() {
                    egui::Window::new(egui::WidgetText::from("hover_info"))
                        .title_bar(false)
                        .resizable(false)
                        .interactable(false)
                        .current_pos(pos)
                        .show(ui.ctx(), |ui| {
                            for (path, count) in folder_info {
                                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
                                ui.label(format!("{} ({})", rel, count));
                            }
                        });
                }
            }
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.window_fill().to_normalized_gamma_f32()
    }
}

fn show_flat_list(
    ui: &mut egui::Ui,
    node: &DirectoryNode,
    root: &Path,
    hovered_files: &RefCell<HashSet<PathBuf>>,
    expanded_folders: &RefCell<HashSet<PathBuf>>,
    hovered_folder: &RefCell<Option<PathBuf>>,
    hovered_folder_pos: &RefCell<egui::Pos2>,
) {
    let currently_hovered = RefCell::new(HashSet::new());
    *hovered_folder.borrow_mut() = None;

    fn collect_folders(node: &DirectoryNode) -> Vec<&DirectoryNode> {
        let mut folders = Vec::new();
        if !node.files.is_empty() {
            folders.push(node);
        }
        for child in &node.children {
            folders.extend(collect_folders(child));
        }
        folders
    }

    let folders = collect_folders(node);
    for folder in folders {
        let rel_path = folder
            .path
            .strip_prefix(root)
            .unwrap_or(&folder.path)
            .to_string_lossy();
let count = folder.files.len();

        let folder_key = folder.path.clone();
        let is_expanded = expanded_folders.borrow().contains(&folder_key);

        let header_text = if is_expanded {
            format!("v ({}) {}", count, rel_path)
        } else {
            format!("> ({}) {}", count, rel_path)
        };

        let response = ui.add(egui::Label::new(header_text).sense(egui::Sense::click()));

        if response.hovered() {
            let rect = response.rect;
            let pos = rect.right_bottom();
            currently_hovered.borrow_mut().insert(folder_key.clone());
            *hovered_folder.borrow_mut() = Some(folder_key.clone());
            *hovered_folder_pos.borrow_mut() = pos;
            let resp = response.clone();
            resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }

        if response.clicked() {
            let mut expanded = expanded_folders.borrow_mut();
            if expanded.contains(&folder_key) {
                expanded.remove(&folder_key);
            } else {
                expanded.insert(folder_key);
            }
        }

        if is_expanded {
            ui.indent(&rel_path, |ui| {
                for (file, others) in &folder.files {
                    let _file_name = file
                        .file_name()
                        .unwrap_or(file.as_os_str())
                        .to_string_lossy();
                    let dup_count = others.len() + 1;
                    ui.horizontal(|ui| {
                        let show_label =
                            |ui: &mut egui::Ui, text: &str, open_path: &Path, id: &Path| {
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

                        ui.label(format!("({})", dup_count));

                        let all_paths: Vec<_> = std::iter::once(file.clone())
                            .chain(others.iter().cloned())
                            .collect();

                        for path in all_paths {
                            let rel = path
                                .strip_prefix(root)
                                .unwrap_or(&path)
                                .to_string_lossy()
                                .to_string();
                            show_label(ui, &rel, path.parent().unwrap_or(&path), &path);
                        }
                    });
                }
            });
        }
    }
}

impl FindDuplicatesApp {
    fn scan(&mut self, path: PathBuf, quick_scan: bool, min_size: u64) {
        self.tree = None;
        self.root = path.clone();
        self.scan_progress = None;
        self.show_settings = false;
        self.folder_info_cache.borrow_mut().clear();

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

    fn populate_folder_cache(&mut self, tree: &DirectoryNode) {
        let mut cache: HashMap<PathBuf, Vec<(PathBuf, usize)>> = HashMap::new();

        fn collect_folders_with_files(node: &DirectoryNode) -> Vec<&DirectoryNode> {
            let mut folders = Vec::new();
            if !node.files.is_empty() {
                folders.push(node);
            }
            for child in &node.children {
                folders.extend(collect_folders_with_files(child));
            }
            folders
        }

        let folders = collect_folders_with_files(tree);

        for folder in folders {
            let folder_path = folder.path.clone();
            let mut folder_counts: HashMap<PathBuf, usize> = HashMap::new();

            for (_file, others) in &folder.files {
                for other in others {
                    if let Some(parent) = other.parent() {
                        if parent != folder_path {
                            *folder_counts.entry(parent.to_path_buf()).or_insert(0) += 1;
                        }
                    }
                }
            }

            let mut sorted: Vec<_> = folder_counts.into_iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(&a.1));

            if !sorted.is_empty() {
                cache.insert(folder_path, sorted);
            }
        }

        *self.folder_info_cache.borrow_mut() = cache;
    }
}