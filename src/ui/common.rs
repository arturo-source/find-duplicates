use std::path::Path;

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Ui, Vec2};

pub fn parse_size(input: &str) -> Option<u64> {
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

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn percent(ratio: f32) -> String {
    let p = ratio * 100.0;
    if p > 99.0 && p < 100.0 {
        // Never round "almost everything" up to a reassuring 100%.
        format!("{p:.1}%")
    } else {
        format!("{p:.0}%")
    }
}

pub fn relative<'a>(path: &'a Path, root: &Path) -> std::borrow::Cow<'a, str> {
    match path.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
        Ok(rel) => rel.to_string_lossy(),
        Err(_) => path.to_string_lossy(),
    }
}

/// Theme-aware palette for statuses and relations.
pub struct Palette {
    pub same: Color32,
    pub contained: Color32,
    pub similar: Color32,
    pub modified: Color32,
    pub unique: Color32,
    pub moved: Color32,
    pub muted: Color32,
}

impl Palette {
    pub fn of(ui: &Ui) -> Self {
        if ui.visuals().dark_mode {
            Self {
                same: Color32::from_rgb(110, 190, 120),
                contained: Color32::from_rgb(100, 160, 230),
                similar: Color32::from_rgb(220, 170, 80),
                modified: Color32::from_rgb(220, 170, 80),
                unique: Color32::from_rgb(230, 100, 100),
                moved: Color32::from_rgb(160, 140, 220),
                muted: Color32::from_gray(140),
            }
        } else {
            Self {
                same: Color32::from_rgb(40, 140, 60),
                contained: Color32::from_rgb(30, 100, 190),
                similar: Color32::from_rgb(180, 120, 0),
                modified: Color32::from_rgb(180, 120, 0),
                unique: Color32::from_rgb(200, 50, 50),
                moved: Color32::from_rgb(110, 80, 190),
                muted: Color32::from_gray(110),
            }
        }
    }
}

/// Paints `text` in one line, dropping characters from the start with a
/// leading "…" when it does not fit (the end of a path is the useful part).
pub fn paint_elided_start(
    painter: &Painter,
    pos: Pos2,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), color);
    let galley = if galley.size().x <= max_width {
        galley
    } else {
        let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let (mut lo, mut hi) = (0usize, starts.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            let candidate = format!("…{}", &text[starts[mid]..]);
            if painter
                .layout_no_wrap(candidate, font.clone(), color)
                .size()
                .x
                <= max_width
            {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        let rest = starts.get(lo).map_or("", |&i| &text[i..]);
        painter.layout_no_wrap(format!("…{rest}"), font, color)
    };
    let rect = Rect::from_min_size(pos, galley.size());
    painter.galley(pos, galley, color);
    rect
}

pub fn paint_badge(painter: &Painter, left_center: Pos2, text: &str, color: Color32) -> Rect {
    let font = FontId::proportional(10.5);
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let size = galley.size() + Vec2::new(10.0, 4.0);
    let rect = Rect::from_min_size(left_center - Vec2::new(0.0, size.y / 2.0), size);
    painter.rect_filled(rect, 3.0, color.gamma_multiply(0.18));
    painter.galley(rect.min + Vec2::new(5.0, 2.0), galley, color);
    rect
}

/// A horizontal bar showing `ratio` filled.
pub fn paint_meter(painter: &Painter, rect: Rect, ratio: f32, color: Color32, track: Color32) {
    painter.rect_filled(rect, 2.0, track);
    let mut fill = rect;
    fill.set_width(rect.width() * ratio.clamp(0.0, 1.0));
    painter.rect_filled(fill, 2.0, color);
}

pub fn paint_disclosure(painter: &Painter, center: Pos2, open: bool, color: Color32) {
    let s = 3.5;
    let points = if open {
        vec![
            center + Vec2::new(-s, -s * 0.5),
            center + Vec2::new(s, -s * 0.5),
            center + Vec2::new(0.0, s * 0.7),
        ]
    } else {
        vec![
            center + Vec2::new(-s * 0.5, -s),
            center + Vec2::new(-s * 0.5, s),
            center + Vec2::new(s * 0.7, 0.0),
        ]
    };
    painter.add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

/// Label that opens `target` in the file manager when clicked.
pub fn path_link(ui: &mut Ui, text: &str, target: &Path) {
    let resp = ui
        .add(
            egui::Label::new(egui::RichText::new(text).monospace())
                .sense(Sense::click())
                .truncate(),
        )
        .on_hover_text(format!(
            "{}\nClick to open in the file manager",
            target.display()
        ))
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        let _ = open::that(target);
    }
}

pub fn text_left(
    painter: &Painter,
    pos: Pos2,
    text: impl ToString,
    size: f32,
    color: Color32,
) -> Rect {
    painter.text(
        pos,
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(size),
        color,
    )
}

pub fn text_right(
    painter: &Painter,
    pos: Pos2,
    text: impl ToString,
    size: f32,
    color: Color32,
) -> Rect {
    painter.text(
        pos,
        Align2::RIGHT_CENTER,
        text,
        FontId::proportional(size),
        color,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1MB"), Some(1024 * 1024));
        assert_eq!(parse_size("64 bytes"), Some(64));
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_count(1234567), "1,234,567");
    }
}
