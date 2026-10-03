use egui::{FontId, Pos2, Sense, Ui, Vec2};
use find_duplicates::scan::ScanResult;

use super::common::*;

const ROW_HEIGHT: f32 = 20.0;

enum Row {
    Group(usize),
    File(usize),
}

#[derive(Default)]
pub struct FilesView {
    search: String,
    /// Group indices sorted by wasted bytes, computed once per scan.
    order: Vec<usize>,
    rows: Vec<Row>,
    rows_for: Option<String>,
}

impl FilesView {
    pub fn reset(&mut self, scan: &ScanResult) {
        let mut order: Vec<usize> = (0..scan.groups.len()).collect();
        let wasted = |g: &Vec<usize>| scan.files[g[0]].size * (g.len() as u64 - 1);
        order.sort_unstable_by(|&x, &y| wasted(&scan.groups[y]).cmp(&wasted(&scan.groups[x])));
        *self = Self {
            order,
            ..Default::default()
        };
    }

    fn rebuild_rows(&mut self, scan: &ScanResult) {
        let needle = self.search.to_lowercase();
        self.rows.clear();
        for &g in &self.order {
            let group = &scan.groups[g];
            if !needle.is_empty()
                && !group.iter().any(|&i| {
                    relative(&scan.files[i].path, &scan.root)
                        .to_lowercase()
                        .contains(&needle)
                })
            {
                continue;
            }
            self.rows.push(Row::Group(g));
            self.rows.extend(group.iter().map(|&i| Row::File(i)));
        }
        self.rows_for = Some(self.search.clone());
    }

    pub fn show(&mut self, ui: &mut Ui, scan: &ScanResult) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Filter by path…")
                    .desired_width(300.0),
            );
            ui.weak(format!(
                "{} groups of identical files · {} wasted",
                format_count(scan.groups.len()),
                format_bytes(scan.wasted_bytes())
            ));
        });
        ui.separator();
        if self.rows_for.as_deref() != Some(self.search.as_str()) {
            self.rebuild_rows(scan);
        }

        let pal = Palette::of(ui);
        egui::ScrollArea::vertical()
            .id_salt("files_scroll")
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, self.rows.len(), |ui, range| {
                for row in &self.rows[range] {
                    let (rect, resp) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), ROW_HEIGHT),
                        Sense::click(),
                    );
                    let painter = ui.painter_at(rect);
                    let y = rect.center().y;
                    match *row {
                        Row::Group(g) => {
                            let group = &scan.groups[g];
                            let size = scan.files[group[0]].size;
                            painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
                            text_left(
                                &painter,
                                Pos2::new(rect.left() + 6.0, y),
                                format!("{} copies · {} each", group.len(), format_bytes(size)),
                                12.0,
                                ui.visuals().strong_text_color(),
                            );
                            text_right(
                                &painter,
                                Pos2::new(rect.right() - 8.0, y),
                                format!("{} wasted", format_bytes(size * (group.len() as u64 - 1))),
                                11.0,
                                pal.unique,
                            );
                        }
                        Row::File(i) => {
                            let path = &scan.files[i].path;
                            if resp.hovered() {
                                painter.rect_filled(
                                    rect,
                                    0.0,
                                    ui.visuals().widgets.hovered.weak_bg_fill,
                                );
                            }
                            paint_elided_start(
                                &painter,
                                Pos2::new(rect.left() + 22.0, y - 7.0),
                                &relative(path, &scan.root),
                                FontId::monospace(11.5),
                                ui.visuals().text_color(),
                                rect.width() - 30.0,
                            );
                            let resp = resp
                                .on_hover_text("Click to open the containing folder")
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if resp.clicked() {
                                if let Some(parent) = path.parent() {
                                    let _ = open::that(parent);
                                }
                            }
                        }
                    }
                }
            });
    }
}
