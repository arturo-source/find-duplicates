use std::collections::HashSet;

use egui::{FontId, Pos2, Rect, Sense, Ui, Vec2};
use find_duplicates::analysis::{CompareTree, FileStatus, FolderPair, FolderSide, Relation};
use find_duplicates::scan::ScanResult;

use super::common::*;

const PAIR_ROW_HEIGHT: f32 = 64.0;
const TREE_ROW_HEIGHT: f32 = 22.0;

#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    Relevance,
    Size,
    Match,
}

#[derive(Clone, Copy, PartialEq)]
enum CompareFilter {
    All,
    Differences,
    UniqueOnly,
}

pub struct PairsView {
    selected: Option<usize>,
    show_identical: bool,
    show_contained: bool,
    show_similar: bool,
    min_similarity: f32,
    search: String,
    sort: SortBy,
    compare: Option<(usize, CompareTree)>,
    expanded: HashSet<usize>,
    filter: CompareFilter,
}

impl Default for PairsView {
    fn default() -> Self {
        Self {
            selected: None,
            show_identical: true,
            show_contained: true,
            show_similar: true,
            min_similarity: 0.0,
            search: String::new(),
            sort: SortBy::Relevance,
            compare: None,
            expanded: HashSet::new(),
            filter: CompareFilter::All,
        }
    }
}

fn relation_style(rel: Relation, pal: &Palette) -> (&'static str, egui::Color32) {
    match rel {
        Relation::Identical => ("IDENTICAL", pal.same),
        Relation::AInB | Relation::BInA => ("CONTAINED", pal.contained),
        Relation::Similar => ("SIMILAR", pal.similar),
    }
}

impl PairsView {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn visible(&self, pair: &FolderPair, scan: &ScanResult) -> bool {
        let rel_ok = match pair.relation() {
            Relation::Identical => self.show_identical,
            Relation::AInB | Relation::BInA => self.show_contained,
            Relation::Similar => self.show_similar,
        };
        if !rel_ok || pair.a.ratio().max(pair.b.ratio()) < self.min_similarity {
            return false;
        }
        if self.search.is_empty() {
            return true;
        }
        let needle = self.search.to_lowercase();
        [&pair.a.path, &pair.b.path]
            .iter()
            .any(|p| relative(p, &scan.root).to_lowercase().contains(&needle))
    }

    pub fn show(&mut self, ui: &mut Ui, scan: &ScanResult, pairs: &[FolderPair]) {
        egui::Panel::left("pairs_list")
            .resizable(true)
            .default_size(420.0)
            .min_size(280.0)
            .show_inside(ui, |ui| self.show_list(ui, scan, pairs));

        egui::CentralPanel::default().show_inside(ui, |ui| match self.selected {
            Some(i) if i < pairs.len() => self.show_compare(ui, scan, &pairs[i], i),
            _ => {
                ui.centered_and_justified(|ui| {
                    ui.weak("Select a folder pair on the left to compare it");
                });
            }
        });
    }

    fn show_list(&mut self, ui: &mut Ui, scan: &ScanResult, pairs: &[FolderPair]) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.toggle_value(&mut self.show_identical, "Identical");
            ui.toggle_value(&mut self.show_contained, "Contained");
            ui.toggle_value(&mut self.show_similar, "Similar");
        });
        ui.horizontal(|ui| {
            ui.label("Min. match");
            ui.add(
                egui::Slider::new(&mut self.min_similarity, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                    .step_by(0.05),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Sort by");
            ui.selectable_value(&mut self.sort, SortBy::Relevance, "Relevance")
                .on_hover_text("Reclaimable size weighted by how much the folders match");
            ui.selectable_value(&mut self.sort, SortBy::Size, "Size");
            ui.selectable_value(&mut self.sort, SortBy::Match, "Match %");
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Filter by path…")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);

        // `pairs` is already sorted by relevance.
        let mut visible: Vec<usize> = (0..pairs.len())
            .filter(|&i| self.visible(&pairs[i], scan))
            .collect();
        match self.sort {
            SortBy::Relevance => {}
            SortBy::Size => visible.sort_by_key(|&i| std::cmp::Reverse(pairs[i].reclaimable())),
            SortBy::Match => visible.sort_by(|&x, &y| {
                let key = |p: &FolderPair| (p.a.ratio().max(p.b.ratio()), p.similarity());
                key(&pairs[y])
                    .partial_cmp(&key(&pairs[x]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
        }
        ui.weak(format!(
            "{} of {} folder pairs",
            format_count(visible.len()),
            format_count(pairs.len())
        ));
        ui.separator();

        if pairs.is_empty() {
            ui.add_space(20.0);
            ui.weak("No similar folders found.");
            return;
        }

        let pal = Palette::of(ui);
        egui::ScrollArea::vertical()
            .id_salt("pairs_scroll")
            .auto_shrink([false, false])
            .show_rows(ui, PAIR_ROW_HEIGHT, visible.len(), |ui, range| {
                for &i in &visible[range] {
                    let (rect, resp) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), PAIR_ROW_HEIGHT),
                        Sense::click(),
                    );
                    if resp.clicked() {
                        self.selected = Some(i);
                    }
                    self.paint_pair_row(
                        ui,
                        rect,
                        &resp,
                        scan,
                        &pairs[i],
                        Some(i) == self.selected,
                        &pal,
                    );
                }
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_pair_row(
        &self,
        ui: &Ui,
        rect: Rect,
        resp: &egui::Response,
        scan: &ScanResult,
        pair: &FolderPair,
        selected: bool,
        pal: &Palette,
    ) {
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        if selected {
            painter.rect_filled(
                rect.shrink(1.0),
                4.0,
                visuals.selection.bg_fill.gamma_multiply(0.6),
            );
        } else if resp.hovered() {
            painter.rect_filled(rect.shrink(1.0), 4.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let text = visuals.text_color();
        let (label, color) = relation_style(pair.relation(), pal);
        let x = rect.left() + 8.0;
        let y0 = rect.top() + 12.0;

        let badge = paint_badge(&painter, Pos2::new(x, y0), label, color);
        text_left(
            &painter,
            Pos2::new(badge.right() + 8.0, y0),
            format_bytes(pair.reclaimable()),
            14.0,
            text,
        );
        text_right(
            &painter,
            Pos2::new(rect.right() - 8.0, y0),
            format!("{} files", format_count(pair.a.files().max(pair.b.files()))),
            11.0,
            pal.muted,
        );

        let side_line = |side: &FolderSide, y: f32, tag: &str| {
            let meter = Rect::from_min_size(
                Pos2::new(rect.right() - 58.0, y - 3.0),
                Vec2::new(50.0, 6.0),
            );
            paint_meter(
                &painter,
                meter,
                side.ratio(),
                color,
                visuals.extreme_bg_color,
            );
            text_right(
                &painter,
                Pos2::new(meter.left() - 6.0, y),
                percent(side.ratio()),
                11.0,
                pal.muted,
            );
            text_left(&painter, Pos2::new(x, y), tag, 11.0, pal.muted);
            let max_w = meter.left() - 46.0 - (x + 16.0);
            paint_elided_start(
                &painter,
                Pos2::new(x + 16.0, y - 7.0),
                &side_label(side, &scan.root),
                FontId::monospace(11.5),
                text,
                max_w,
            );
        };
        side_line(&pair.a, rect.top() + 32.0, "A");
        side_line(&pair.b, rect.top() + 50.0, "B");

        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            visuals.widgets.noninteractive.bg_stroke,
        );
    }

    fn show_compare(&mut self, ui: &mut Ui, scan: &ScanResult, pair: &FolderPair, idx: usize) {
        if self.compare.as_ref().is_none_or(|(i, _)| *i != idx) {
            self.compare = Some((idx, CompareTree::build(scan, pair)));
            self.expanded.clear();
        }
        let pal = Palette::of(ui);

        ui.add_space(4.0);
        ui.columns(2, |cols| {
            side_card(&mut cols[0], scan, "A", &pair.a, &pal);
            side_card(&mut cols[1], scan, "B", &pair.b, &pal);
        });
        ui.add_space(6.0);
        verdict(ui, pair, &pal);
        ui.add_space(6.0);

        let Some((_, tree)) = self.compare.take() else {
            return;
        };
        let root = &tree.nodes[0];
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.filter, CompareFilter::All, "All files");
            ui.selectable_value(
                &mut self.filter,
                CompareFilter::Differences,
                format!("Differences ({})", format_count(root.counts.differences())),
            );
            ui.selectable_value(
                &mut self.filter,
                CompareFilter::UniqueOnly,
                format!(
                    "Only on one side ({})",
                    format_count(root.counts.unique_a + root.counts.unique_b)
                ),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Collapse all").clicked() {
                    self.expanded.clear();
                }
                if ui.small_button("Expand all").clicked() {
                    self.expanded = (0..tree.nodes.len())
                        .filter(|&i| tree.nodes[i].is_dir())
                        .collect();
                }
            });
        });
        legend(ui, &pal);
        ui.separator();
        let (header, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 14.0), Sense::hover());
        let y = header.center().y;
        let right = header.right() - ui.spacing().scroll.bar_width - 4.0;
        text_left(
            ui.painter(),
            Pos2::new(header.left() + 4.0, y),
            "Name",
            11.0,
            pal.muted,
        );
        text_right(
            ui.painter(),
            Pos2::new(right - 98.0, y),
            "A",
            11.0,
            pal.muted,
        );
        text_right(
            ui.painter(),
            Pos2::new(right - 8.0, y),
            "B",
            11.0,
            pal.muted,
        );

        let rows = self.visible_rows(&tree);
        let mut toggled = None;
        egui::ScrollArea::vertical()
            .id_salt(("compare_scroll", idx))
            .auto_shrink([false, false])
            .show_rows(ui, TREE_ROW_HEIGHT, rows.len(), |ui, range| {
                for &n in &rows[range] {
                    if let Some(t) = self.paint_tree_row(ui, scan, &tree, n, &pal) {
                        toggled = Some(t);
                    }
                }
            });
        if let Some(n) = toggled {
            if !self.expanded.remove(&n) {
                self.expanded.insert(n);
            }
        }
        self.compare = Some((idx, tree));
    }

    fn keep(&self, tree: &CompareTree, n: usize) -> bool {
        let c = &tree.nodes[n].counts;
        match self.filter {
            CompareFilter::All => true,
            CompareFilter::Differences => c.differences() > 0,
            CompareFilter::UniqueOnly => c.unique_a + c.unique_b > 0,
        }
    }

    fn visible_rows(&self, tree: &CompareTree) -> Vec<usize> {
        let mut rows = Vec::new();
        let mut stack: Vec<usize> = tree.nodes[0].children.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            if !self.keep(tree, n) {
                continue;
            }
            rows.push(n);
            if self.expanded.contains(&n) {
                stack.extend(tree.nodes[n].children.iter().rev().copied());
            }
        }
        rows
    }

    /// Returns the node to toggle when a directory row is clicked.
    fn paint_tree_row(
        &self,
        ui: &mut Ui,
        scan: &ScanResult,
        tree: &CompareTree,
        n: usize,
        pal: &Palette,
    ) -> Option<usize> {
        let node = &tree.nodes[n];
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), TREE_ROW_HEIGHT),
            Sense::click(),
        );
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        if resp.hovered() {
            painter.rect_filled(rect, 2.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let text = visuals.text_color();
        let y = rect.center().y;
        let x = rect.left() + 4.0 + (node.depth as f32 - 1.0) * 16.0;
        let col_b = rect.right() - 90.0;
        let col_a = col_b - 90.0;
        let col_status = col_a - 200.0;

        let mut toggle = None;
        if node.is_dir() {
            let open = self.expanded.contains(&n);
            paint_disclosure(&painter, Pos2::new(x + 5.0, y), open, pal.muted);
            text_left(
                &painter,
                Pos2::new(x + 14.0, y),
                format!("{}/", node.name),
                13.0,
                text,
            );
            let c = node.counts;
            let total = c.same + c.modified + c.moved_a + c.moved_b + c.unique_a + c.unique_b;
            let mut parts = Vec::new();
            if c.differences() == 0 {
                parts.push((format!("{} identical", format_count(total)), pal.same));
            } else {
                for (count, label, color) in [
                    (c.same, "same", pal.same),
                    (c.modified, "modified", pal.modified),
                    (c.moved_a + c.moved_b, "moved", pal.moved),
                    (c.unique_a, "only A", pal.unique),
                    (c.unique_b, "only B", pal.unique),
                ] {
                    if count > 0 {
                        parts.push((format!("{} {label}", format_count(count)), color));
                    }
                }
            }
            let mut px = col_status;
            for (label, color) in parts {
                let r = text_left(&painter, Pos2::new(px, y), label, 11.0, color);
                px = r.right() + 8.0;
            }
            if resp.clicked() {
                toggle = Some(n);
            }
        } else {
            let status = node.status.unwrap();
            let (label, color) = match status {
                FileStatus::Same => ("same", pal.same),
                FileStatus::Modified => ("modified", pal.modified),
                FileStatus::MovedA => ("elsewhere in B", pal.moved),
                FileStatus::MovedB => ("elsewhere in A", pal.moved),
                FileStatus::UniqueA => ("only in A", pal.unique),
                FileStatus::UniqueB => ("only in B", pal.unique),
            };
            painter.circle_filled(Pos2::new(x + 5.0, y), 3.5, color);
            paint_elided_start(
                &painter,
                Pos2::new(x + 14.0, y - 8.0),
                &node.name,
                FontId::proportional(13.0),
                text,
                (col_status - 12.0 - (x + 14.0)).max(40.0),
            );
            text_left(&painter, Pos2::new(col_status, y), label, 11.0, color);
            let size =
                |f: Option<usize>| f.map_or("—".to_owned(), |i| format_bytes(scan.files[i].size));
            text_right(
                &painter,
                Pos2::new(col_b - 10.0, y),
                size(node.file_a),
                11.0,
                pal.muted,
            );
            text_right(
                &painter,
                Pos2::new(rect.right() - 8.0, y),
                size(node.file_b),
                11.0,
                pal.muted,
            );

            let target = node.file_a.or(node.file_b).map(|i| &scan.files[i].path);
            if let Some(path) = target {
                let resp = resp.on_hover_text(path.display().to_string());
                if resp.double_clicked() {
                    if let Some(parent) = path.parent() {
                        let _ = open::that(parent);
                    }
                }
                resp.context_menu(|ui| {
                    for (label, f) in [
                        ("Open folder in A", node.file_a),
                        ("Open folder in B", node.file_b),
                    ] {
                        if let Some(i) = f {
                            if ui.button(label).clicked() {
                                if let Some(parent) = scan.files[i].path.parent() {
                                    let _ = open::that(parent);
                                }
                            }
                        }
                    }
                });
            }
        }
        toggle
    }
}

/// Folder path relative to the scan root, noting a nested copy left out.
fn side_label(side: &FolderSide, root: &std::path::Path) -> String {
    let label = relative(&side.path, root);
    if side.excluded.is_some() {
        format!("{label} (without nested copy)")
    } else {
        label.into_owned()
    }
}

fn side_card(ui: &mut Ui, scan: &ScanResult, tag: &str, side: &FolderSide, pal: &Palette) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.strong(tag);
            path_link(ui, &relative(&side.path, &scan.root), &side.path);
        });
        if let Some(excluded) = &side.excluded {
            ui.weak(format!(
                "Without its nested copy {}",
                relative(excluded, &side.path)
            ));
        }
        ui.horizontal(|ui| {
            ui.weak(format!(
                "{} files · {}",
                format_count(side.files()),
                format_bytes(side.bytes)
            ));
        });
        let other = if tag == "A" { "B" } else { "A" };
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 8.0), Sense::hover());
        let color = if side.ratio() >= 1.0 {
            pal.same
        } else {
            pal.similar
        };
        paint_meter(
            ui.painter(),
            rect,
            side.ratio(),
            color,
            ui.visuals().extreme_bg_color,
        );
        ui.label(format!(
            "{} of its files are in {other}",
            percent(side.ratio())
        ));
        if side.unique_files() > 0 {
            ui.colored_label(
                pal.unique,
                format!(
                    "{} files ({}) exist only here",
                    format_count(side.unique_files()),
                    format_bytes(side.bytes - side.shared_bytes)
                ),
            );
        } else {
            ui.colored_label(pal.same, "Nothing unique: fully backed up");
        }
    });
}

fn verdict(ui: &mut Ui, pair: &FolderPair, pal: &Palette) {
    let (text, color) = match pair.relation() {
        Relation::Identical => (
            format!("Both folders have the same files. Deleting either frees {}.", format_bytes(pair.a.bytes)),
            pal.same,
        ),
        Relation::AInB => (
            format!("Everything in A also exists in B. A can be deleted without losing data ({}).", format_bytes(pair.a.bytes)),
            pal.contained,
        ),
        Relation::BInA => (
            format!("Everything in B also exists in A. B can be deleted without losing data ({}).", format_bytes(pair.b.bytes)),
            pal.contained,
        ),
        Relation::Similar => (
            format!(
                "The folders overlap {}: {} files in A and {} in B have no copy on the other side. Merge them before deleting.",
                percent(pair.similarity()),
                format_count(pair.a.unique_files()),
                format_count(pair.b.unique_files())
            ),
            pal.similar,
        ),
    };
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .corner_radius(4.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.colored_label(color, text);
        });
}

fn legend(ui: &mut Ui, pal: &Palette) {
    ui.horizontal(|ui| {
        for (label, color) in [
            ("same", pal.same),
            ("modified (same name, different content)", pal.modified),
            ("moved/renamed", pal.moved),
            ("not copied", pal.unique),
        ] {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 3.5, color);
            ui.small(label);
            ui.add_space(6.0);
        }
    });
}
