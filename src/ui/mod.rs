mod common;
mod files;
mod pairs;
mod treemap;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use find_duplicates::analysis::{find_folder_pairs, FolderPair};
use find_duplicates::cache::HashCache;
use find_duplicates::scan::{scan, Progress, ScanOptions, ScanResult, Stage};

use common::*;

const DEFAULT_IGNORE: &[&str] = &[".git", "node_modules", "__pycache__", ".DS_Store"];

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Folders,
    Files,
    Treemap,
}

struct Settings {
    patterns: Vec<String>,
    new_pattern: String,
    quick_scan: bool,
    min_size: u64,
    size_input: String,
    use_cache: bool,
}

struct Job {
    progress: Arc<Progress>,
    rx: mpsc::Receiver<Option<(ScanResult, Vec<FolderPair>)>>,
    started: Instant,
}

struct Results {
    scan: ScanResult,
    pairs: Vec<FolderPair>,
    elapsed: Duration,
}

pub struct FindDuplicatesApp {
    settings: Settings,
    show_settings: bool,
    root: Option<PathBuf>,
    job: Option<Job>,
    results: Option<Results>,
    tab: Tab,
    pairs_view: pairs::PairsView,
    files_view: files::FilesView,
    treemap_view: treemap::TreemapView,
    status: Option<String>,
}

impl FindDuplicatesApp {
    pub fn new(ctx: egui::Context, initial: Option<PathBuf>) -> Self {
        match dark_light::detect() {
            Ok(dark_light::Mode::Dark) => ctx.set_theme(egui::Theme::Dark),
            _ => ctx.set_theme(egui::Theme::Light),
        }
        let mut app = Self {
            settings: Settings {
                patterns: DEFAULT_IGNORE.iter().map(|s| s.to_string()).collect(),
                new_pattern: String::new(),
                quick_scan: true,
                min_size: 64,
                size_input: "64 bytes".into(),
                use_cache: true,
            },
            show_settings: false,
            root: None,
            job: None,
            results: None,
            tab: Tab::Folders,
            pairs_view: Default::default(),
            files_view: Default::default(),
            treemap_view: Default::default(),
            status: None,
        };
        if let Some(root) = initial.filter(|p| p.is_dir()) {
            app.start_scan(&ctx, root);
        }
        app
    }

    fn start_scan(&mut self, ctx: &egui::Context, root: PathBuf) {
        if let Some(job) = &self.job {
            job.progress.cancel.store(true, Ordering::Relaxed);
        }
        self.root = Some(root.clone());
        self.results = None;
        self.status = None;
        self.show_settings = false;

        let opts = ScanOptions {
            ignore: self
                .settings
                .patterns
                .iter()
                .cloned()
                .collect::<HashSet<_>>(),
            min_size: self.settings.min_size,
            quick_scan: self.settings.quick_scan,
            cache_path: self
                .settings
                .use_cache
                .then(HashCache::default_path)
                .flatten(),
        };
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let p = progress.clone();
        thread::spawn(move || {
            let result = scan(&root, &opts, &p).map(|scan| {
                p.set_stage(Stage::Analyzing, 0);
                let pairs = find_folder_pairs(&scan);
                (scan, pairs)
            });
            let _ = tx.send(if p.cancelled() { None } else { result });
            ctx.request_repaint();
        });
        self.job = Some(Job {
            progress,
            rx,
            started: Instant::now(),
        });
    }

    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        let Ok(msg) = job.rx.try_recv() else { return };
        let elapsed = job.started.elapsed();
        self.job = None;
        match msg {
            Some((scan, pairs)) => {
                self.pairs_view.reset();
                self.files_view.reset(&scan);
                self.treemap_view.reset(&scan);
                self.results = Some(Results {
                    scan,
                    pairs,
                    elapsed,
                });
            }
            None => self.status = Some("Scan cancelled".into()),
        }
    }

    fn pick_folder(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.start_scan(ctx, path);
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Select folder…").clicked() {
                self.pick_folder(ui.ctx());
            }
            let can_rescan = self.root.is_some() && self.job.is_none();
            if ui
                .add_enabled(can_rescan, egui::Button::new("Rescan"))
                .clicked()
            {
                if let Some(root) = self.root.clone() {
                    self.start_scan(ui.ctx(), root);
                }
            }
            if ui.button("Settings").clicked() {
                self.show_settings = true;
            }
            if self.results.is_some() && self.job.is_none() {
                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Folders, "Similar folders");
                ui.selectable_value(&mut self.tab, Tab::Files, "Duplicate files");
                ui.selectable_value(&mut self.tab, Tab::Treemap, "Treemap");
            }
            if let Some(root) = &self.root {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    path_link(ui, &root.to_string_lossy(), root);
                });
            }
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(job) = &self.job {
                let p = &job.progress;
                ui.spinner();
                ui.label(p.stage().label());
                if let Some(f) = p.fraction() {
                    ui.add(
                        egui::ProgressBar::new(f)
                            .desired_width(160.0)
                            .show_percentage(),
                    );
                } else if p.stage() == Stage::Listing {
                    ui.weak(format!(
                        "{} files",
                        format_count(p.done.load(Ordering::Relaxed) as usize)
                    ));
                }
                if ui.small_button("Cancel").clicked() {
                    p.cancel.store(true, Ordering::Relaxed);
                }
            } else if let Some(r) = &self.results {
                let s = &r.scan;
                let hashed: usize = s.groups.iter().map(|g| g.len()).sum();
                ui.weak(format!(
                    "{} files · {} duplicated · {} wasted · {} folder pairs · {:.1}s{}",
                    format_count(s.files.len()),
                    format_count(hashed),
                    format_bytes(s.wasted_bytes()),
                    format_count(r.pairs.len()),
                    r.elapsed.as_secs_f32(),
                    if s.cache_hits > 0 {
                        format!(" · {} hashes from cache", format_count(s.cache_hits))
                    } else {
                        String::new()
                    },
                ));
            } else if let Some(status) = &self.status {
                ui.weak(status);
            }
        });
    }

    fn scanning_view(&self, ui: &mut egui::Ui, job: &Job) {
        let current = job.progress.stage();
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.heading("Scanning…");
            ui.add_space(12.0);
        });
        let pal = Palette::of(ui);
        let width = 280.0;
        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
            ui.vertical(|ui| {
                ui.set_width(width);
                for stage in Stage::ALL {
                    if stage == Stage::FullHash && self.settings.quick_scan {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        if (stage as u8) < (current as u8) {
                            ui.colored_label(pal.same, "[x]");
                            ui.colored_label(pal.same, stage.label());
                        } else if stage == current {
                            ui.spinner();
                            ui.strong(stage.label());
                        } else {
                            ui.colored_label(pal.muted, "[ ]");
                            ui.colored_label(pal.muted, stage.label());
                        }
                    });
                    if stage == current {
                        if let Some(f) = job.progress.fraction() {
                            ui.add(egui::ProgressBar::new(f).show_percentage());
                        }
                    }
                }
            });
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.heading("Find duplicate files and folders");
            ui.add_space(6.0);
            ui.weak("Detects folders that are copies of each other, even when they are only partly the same,\nso old backups of backups can be cleaned up safely.");
            ui.add_space(16.0);
            if ui.add(egui::Button::new("Select folder to scan…").min_size(egui::vec2(200.0, 32.0))).clicked() {
                self.pick_folder(ui.ctx());
            }
            if let Some(status) = &self.status {
                ui.add_space(8.0);
                ui.weak(status);
            }
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::new("settings")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Settings");
            ui.add_space(8.0);
            let s = &mut self.settings;

            ui.checkbox(&mut s.quick_scan, "Quick scan")
                .on_hover_text("Compare only the first and last 4 KB of files with the same size.\nMuch faster, but verify before deleting anything.");
            ui.checkbox(&mut s.use_cache, "Cache hashes between scans");
            if let Some(path) = HashCache::default_path() {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    ui.weak(path.display().to_string());
                });
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Min file size:");
                if ui.text_edit_singleline(&mut s.size_input).changed() {
                    if let Some(bytes) = parse_size(&s.size_input) {
                        s.min_size = bytes;
                    }
                }
            });
            match parse_size(&s.size_input) {
                Some(bytes) => ui.weak(format!("{} bytes", format_count(bytes as usize))),
                None => ui.colored_label(Palette::of(ui).similar, "e.g. 1MB, 50KB, 64 bytes"),
            };

            ui.add_space(6.0);
            ui.label("Ignore folders/files named:");
            let mut remove = None;
            for (i, pattern) in s.patterns.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("x").clicked() {
                        remove = Some(i);
                    }
                    ui.monospace(pattern);
                });
            }
            if let Some(i) = remove {
                s.patterns.remove(i);
            }
            ui.horizontal(|ui| {
                let resp = ui.text_edit_singleline(&mut s.new_pattern);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Add").clicked() || enter) && !s.new_pattern.trim().is_empty() {
                    s.patterns.push(s.new_pattern.trim().to_owned());
                    s.new_pattern.clear();
                }
            });

            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.button("Close").clicked())
                .inner
        });
        if modal.inner || modal.should_close() {
            self.show_settings = false;
        }
    }
}

impl eframe::App for FindDuplicatesApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_job();
        if self.job.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        egui::Panel::top("top_bar").show_inside(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status_bar").show_inside(ui, |ui| self.status_bar(ui));

        egui::CentralPanel::no_frame().show_inside(ui, |ui| {
            if let Some(job) = &self.job {
                self.scanning_view(ui, job);
            } else if let Some(r) = &self.results {
                match self.tab {
                    Tab::Folders => self.pairs_view.show(ui, &r.scan, &r.pairs),
                    Tab::Files => {
                        egui::CentralPanel::default()
                            .show_inside(ui, |ui| self.files_view.show(ui, &r.scan));
                    }
                    Tab::Treemap => {
                        egui::CentralPanel::default()
                            .show_inside(ui, |ui| self.treemap_view.show(ui, &r.scan));
                    }
                }
            } else {
                self.welcome(ui);
            }
        });

        if self.show_settings {
            self.settings_window(ui.ctx());
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.window_fill().to_normalized_gamma_f32()
    }
}
