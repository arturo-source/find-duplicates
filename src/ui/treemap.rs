//! Squarified treemap of the scanned folder, coloured by how much of each
//! folder is duplicated somewhere else.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use find_duplicates::scan::ScanResult;

use super::common::*;

const MAX_DEPTH: usize = 4;
const HEADER: f32 = 16.0;

struct Node {
    name: String,
    path: PathBuf,
    size: u64,
    dup: u64,
    children: Vec<usize>,
    is_dir: bool,
}

#[derive(Default)]
pub struct TreemapView {
    nodes: Vec<Node>,
    /// Current zoom path, as node indices from the root.
    trail: Vec<usize>,
    layout: Vec<(usize, Rect, usize)>,
    layout_key: Option<(usize, Rect)>,
}

impl TreemapView {
    pub fn reset(&mut self, scan: &ScanResult) {
        let mut nodes = vec![Node {
            name: scan
                .root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path: scan.root.clone(),
            size: 0,
            dup: 0,
            children: Vec::new(),
            is_dir: true,
        }];
        let mut dirs: HashMap<PathBuf, usize> = HashMap::new();
        dirs.insert(scan.root.clone(), 0);

        fn ensure_dir(
            p: &Path,
            nodes: &mut Vec<Node>,
            dirs: &mut HashMap<PathBuf, usize>,
        ) -> usize {
            if let Some(&i) = dirs.get(p) {
                return i;
            }
            let parent = ensure_dir(p.parent().unwrap(), nodes, dirs);
            let idx = nodes.len();
            nodes.push(Node {
                name: p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: p.to_path_buf(),
                size: 0,
                dup: 0,
                children: Vec::new(),
                is_dir: true,
            });
            nodes[parent].children.push(idx);
            dirs.insert(p.to_path_buf(), idx);
            idx
        }

        for (i, f) in scan.files.iter().enumerate() {
            let Some(parent) = f.path.parent() else {
                continue;
            };
            if !parent.starts_with(&scan.root) {
                continue;
            }
            let dup = if scan.is_duplicate(i) { f.size } else { 0 };
            let dir = ensure_dir(parent, &mut nodes, &mut dirs);
            let idx = nodes.len();
            nodes.push(Node {
                name: f
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: f.path.clone(),
                size: f.size,
                dup,
                children: Vec::new(),
                is_dir: false,
            });
            nodes[dir].children.push(idx);
            // Accumulate up to the root.
            let mut p = Some(parent);
            while let Some(d) = p.and_then(|d| dirs.get(d).map(|&i| (d, i))) {
                nodes[d.1].size += f.size;
                nodes[d.1].dup += dup;
                p = d.0.parent();
            }
        }
        for i in 0..nodes.len() {
            let mut children = std::mem::take(&mut nodes[i].children);
            children.sort_unstable_by(|&a, &b| nodes[b].size.cmp(&nodes[a].size));
            nodes[i].children = children;
        }
        *self = Self {
            nodes,
            trail: vec![0],
            ..Default::default()
        };
    }

    fn current(&self) -> usize {
        *self.trail.last().unwrap_or(&0)
    }

    fn compute_layout(&mut self, rect: Rect) {
        let key = (self.current(), rect);
        if self.layout_key == Some(key) {
            return;
        }
        self.layout.clear();
        self.layout_node(self.current(), rect, 0);
        self.layout_key = Some(key);
    }

    fn layout_node(&mut self, node: usize, rect: Rect, depth: usize) {
        let items: Vec<(usize, f64)> = self.nodes[node]
            .children
            .iter()
            .filter(|&&c| self.nodes[c].size > 0)
            .map(|&c| (c, self.nodes[c].size as f64))
            .collect();
        for (child, r) in squarify(&items, rect) {
            if r.width() < 2.0 || r.height() < 2.0 {
                continue;
            }
            self.layout.push((child, r, depth));
            let inner =
                Rect::from_min_max(r.min + Vec2::new(2.0, HEADER), r.max - Vec2::splat(2.0));
            if self.nodes[child].is_dir
                && depth + 1 < MAX_DEPTH
                && inner.width() > 30.0
                && inner.height() > 24.0
            {
                self.layout_node(child, inner, depth + 1);
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, scan: &ScanResult) {
        if self.nodes.is_empty() {
            return;
        }
        let pal = Palette::of(ui);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let mut go_to = None;
            for (pos, &n) in self.trail.iter().enumerate() {
                if pos > 0 {
                    ui.weak("/");
                }
                let name = self.nodes[n].name.clone();
                if ui.link(name).clicked() {
                    go_to = Some(pos);
                }
            }
            if let Some(pos) = go_to {
                self.trail.truncate(pos + 1);
            }
            let cur = &self.nodes[self.current()];
            ui.weak(format!(
                "· {} · {} duplicated ({})",
                format_bytes(cur.size),
                format_bytes(cur.dup),
                percent(if cur.size > 0 {
                    cur.dup as f32 / cur.size as f32
                } else {
                    0.0
                })
            ));
        });
        ui.horizontal(|ui| {
            ui.small("Duplicated content:");
            for (ratio, label) in [(0.0, "0%"), (0.5, "50%"), (1.0, "100%")] {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 10.0), Sense::hover());
                ui.painter()
                    .rect_filled(rect, 2.0, heat(ratio, ui.visuals().dark_mode));
                ui.small(label);
            }
            ui.small(" · click a folder to zoom in, right-click to go up");
        });
        ui.separator();

        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
        self.compute_layout(rect);
        let painter = ui.painter_at(rect);
        let dark = ui.visuals().dark_mode;
        let hover = resp.hover_pos();
        let mut hovered: Option<usize> = None;

        for &(n, r, _depth) in &self.layout {
            let node = &self.nodes[n];
            let ratio = if node.size > 0 {
                node.dup as f32 / node.size as f32
            } else {
                0.0
            };
            let fill = heat(ratio, dark);
            painter.rect_filled(r, 2.0, fill);
            painter.rect_stroke(
                r,
                2.0,
                Stroke::new(1.0, ui.visuals().extreme_bg_color),
                egui::StrokeKind::Inside,
            );
            if hover.is_some_and(|p| r.contains(p)) {
                hovered = Some(n);
            }
            if r.width() > if node.is_dir { 40.0 } else { 70.0 } && r.height() > 14.0 {
                let label = if node.is_dir {
                    format!("{}/ {}", node.name, format_bytes(node.size))
                } else {
                    node.name.clone()
                };
                paint_elided_start(
                    &painter,
                    r.min + Vec2::new(4.0, 1.0),
                    &label,
                    FontId::proportional(11.0),
                    Color32::from_gray(if dark { 235 } else { 20 }),
                    r.width() - 8.0,
                );
            }
        }

        // Deepest rect under the cursor wins: it was pushed last.
        if let Some(n) = hovered {
            let node = &self.nodes[n];
            if let Some(&(_, r, _)) = self.layout.iter().rev().find(|(i, _, _)| *i == n) {
                painter.rect_stroke(
                    r,
                    2.0,
                    Stroke::new(2.0, ui.visuals().strong_text_color()),
                    egui::StrokeKind::Inside,
                );
            }
            let ratio = if node.size > 0 {
                node.dup as f32 / node.size as f32
            } else {
                0.0
            };
            let resp = resp.clone().on_hover_ui_at_pointer(|ui| {
                ui.monospace(relative(&node.path, &scan.root));
                ui.label(format!(
                    "{} · {} duplicated ({})",
                    format_bytes(node.size),
                    format_bytes(node.dup),
                    percent(ratio)
                ));
                if node.is_dir {
                    ui.colored_label(pal.muted, "Click to zoom in");
                } else {
                    ui.colored_label(pal.muted, "Double-click to open its folder");
                }
            });
            if resp.clicked() {
                // Zoom into the top-level folder under the cursor.
                let pointer = hover.unwrap_or_default();
                let top = self
                    .layout
                    .iter()
                    .find(|(i, r, d)| *d == 0 && self.nodes[*i].is_dir && r.contains(pointer));
                if let Some(&(t, _, _)) = top {
                    self.trail.push(t);
                }
            }
            if resp.double_clicked() && !node.is_dir {
                if let Some(parent) = node.path.parent() {
                    let _ = open::that(parent);
                }
            }
        }
        if resp.secondary_clicked() && self.trail.len() > 1 {
            self.trail.pop();
        }
    }
}

fn heat(ratio: f32, dark: bool) -> Color32 {
    let (cold, hot) = if dark {
        ([52.0, 78.0, 92.0], [160.0, 72.0, 64.0])
    } else {
        ([170.0, 205.0, 215.0], [235.0, 120.0, 105.0])
    };
    let t = ratio.clamp(0.0, 1.0);
    let c = |i: usize| (cold[i] + (hot[i] - cold[i]) * t) as u8;
    Color32::from_rgb(c(0), c(1), c(2))
}

/// Squarified treemap layout (Bruls et al.). `items` must be sorted by size, descending.
fn squarify(items: &[(usize, f64)], rect: Rect) -> Vec<(usize, Rect)> {
    let total: f64 = items.iter().map(|i| i.1).sum();
    let mut out = Vec::with_capacity(items.len());
    if total <= 0.0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return out;
    }
    let scale = (rect.width() as f64 * rect.height() as f64) / total;
    let mut free = rect;
    let mut row: Vec<(usize, f64)> = Vec::new();

    let worst = |row: &[(usize, f64)], side: f64| -> f64 {
        let sum: f64 = row.iter().map(|r| r.1).sum();
        let (min, max) = row
            .iter()
            .fold((f64::MAX, 0.0f64), |(lo, hi), r| (lo.min(r.1), hi.max(r.1)));
        let s2 = sum * sum;
        ((side * side * max) / s2).max(s2 / (side * side * min))
    };

    let flush = |row: &mut Vec<(usize, f64)>, free: &mut Rect, out: &mut Vec<(usize, Rect)>| {
        let sum: f64 = row.iter().map(|r| r.1).sum();
        let horizontal = free.width() >= free.height();
        if horizontal {
            // Column on the left.
            let w = (sum / free.height() as f64) as f32;
            let mut y = free.top();
            for &(id, a) in row.iter() {
                let h = (a / w as f64) as f32;
                out.push((
                    id,
                    Rect::from_min_size(Pos2::new(free.left(), y), Vec2::new(w, h)),
                ));
                y += h;
            }
            free.min.x += w;
        } else {
            let h = (sum / free.width() as f64) as f32;
            let mut x = free.left();
            for &(id, a) in row.iter() {
                let w = (a / h as f64) as f32;
                out.push((
                    id,
                    Rect::from_min_size(Pos2::new(x, free.top()), Vec2::new(w, h)),
                ));
                x += w;
            }
            free.min.y += h;
        }
        row.clear();
    };

    for &(id, size) in items {
        let area = size * scale;
        let side = free.width().min(free.height()) as f64;
        let mut candidate = row.clone();
        candidate.push((id, area));
        if row.is_empty() || worst(&candidate, side) <= worst(&row, side) {
            row = candidate;
        } else {
            flush(&mut row, &mut free, &mut out);
            row.push((id, area));
        }
    }
    if !row.is_empty() {
        flush(&mut row, &mut free, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squarify_covers_area() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0));
        let items = [
            (0, 6.0),
            (1, 6.0),
            (2, 4.0),
            (3, 3.0),
            (4, 2.0),
            (5, 2.0),
            (6, 1.0),
        ];
        let out = squarify(&items, rect);
        assert_eq!(out.len(), items.len());
        let area: f32 = out.iter().map(|(_, r)| r.area()).sum();
        assert!((area - rect.area()).abs() < 1.0);
        for (_, r) in &out {
            assert!(rect.expand(0.5).contains_rect(*r));
        }
    }
}
