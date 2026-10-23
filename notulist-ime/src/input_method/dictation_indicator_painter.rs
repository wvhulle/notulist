use std::f32::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{point, Font, FontArc, GlyphId, PxScale, ScaleFont};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

use super::indicator_phase::{IndicatorFrame, IndicatorPhase};
use crate::ime_config::{LevelMeterConfig, PopupConfig, Rgba};

const SPINNER_DOTS: usize = 6;
const MIC_COLOUR: Rgba = [0xa0, 0xa0, 0xa8, 0xff];
const INACTIVE_SPINNER_COLOUR: Rgba = [0x66, 0x66, 0x6e, 0xff];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl RectF {
    fn contains(&self, point: SurfacePoint) -> bool {
        let (x, y) = (point.x as f32, point.y as f32);
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfacePoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PillLayout {
    pub size: PixelSize,
    pub mic_button: Option<RectF>,
    pub status: RectF,
    pub label: Option<RectF>,
}

struct ElementRow {
    next_x: f32,
    spacing: f32,
}

impl ElementRow {
    fn place(&mut self, width: f32) -> f32 {
        let x = self.next_x;
        self.next_x += width + self.spacing;
        x
    }
}

impl PillLayout {
    pub fn from_popup_config(popup: &PopupConfig, label_width: f32) -> Self {
        let height = popup.pill_height_px as f32;
        let padding = popup.inner_padding_px as f32;
        let inner_height = (height - 2.0 * padding).max(0.0);
        let mut row = ElementRow {
            next_x: padding,
            spacing: popup.spacing_between_elements_px as f32,
        };

        let mic_button = popup.mic_button.show_mic_button.then(|| {
            let size = popup.mic_button.mic_button_size_px as f32;
            RectF {
                x: row.place(size),
                y: (height - size) / 2.0,
                width: size,
                height: size,
            }
        });

        let status_width = match popup.level_meter.show_level_meter {
            true => meter_width(&popup.level_meter),
            false => inner_height,
        };
        let status = RectF {
            x: row.place(status_width),
            y: padding,
            width: status_width,
            height: inner_height,
        };

        let label = (popup.phase_label.show_phase_label && label_width > 0.0).then(|| RectF {
            x: row.place(label_width),
            y: padding,
            width: label_width,
            height: inner_height,
        });

        let last_element = label.unwrap_or(status);
        let width = (last_element.x + last_element.width + padding).ceil() as u32;
        Self {
            size: PixelSize {
                width,
                height: popup.pill_height_px,
            },
            mic_button,
            status,
            label,
        }
    }
}

fn meter_width(meter: &LevelMeterConfig) -> f32 {
    let bars = meter.bar_count as f32;
    let bar_gaps = (bars - 1.0).max(0.0);
    bars * meter.bar_width_px as f32 + bar_gaps * meter.spacing_between_bars_px as f32
}

pub fn parse_hex(input: &str) -> Option<Rgba> {
    let digits = input.trim().trim_start_matches('#');
    if !matches!(digits.len(), 6 | 8) || !digits.is_ascii() {
        return None;
    }
    let channel = |index: usize| u8::from_str_radix(&digits[index..index + 2], 16).ok();
    let alpha = match digits.len() {
        8 => channel(6)?,
        _ => 0xff,
    };
    Some([channel(0)?, channel(2)?, channel(4)?, alpha])
}

fn load_font(path: PathBuf) -> Option<FontArc> {
    fs::read(path)
        .ok()
        .and_then(|data| FontArc::try_from_vec(data).ok())
}

fn default_font_path() -> Option<PathBuf> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", "sans"])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .to_owned();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

pub fn resolve_font(override_path: Option<&Path>) -> Option<FontArc> {
    if let Some(path) = override_path {
        return load_font(path.to_path_buf());
    }
    static DEFAULT: OnceLock<Option<FontArc>> = OnceLock::new();
    DEFAULT
        .get_or_init(|| default_font_path().and_then(load_font))
        .clone()
}

pub struct PaintedPill {
    pub size: PixelSize,
    pub rgba: Vec<u8>,
}

pub struct PillPainter {
    popup: PopupConfig,
    font: Option<FontArc>,
    layout: PillLayout,
}

impl PillPainter {
    pub fn new(popup: &PopupConfig, font: Option<FontArc>) -> Self {
        let label_width = font
            .as_ref()
            .map_or(0.0, |font| widest_label_width(font, popup));
        let layout = PillLayout::from_popup_config(popup, label_width);
        Self {
            popup: popup.clone(),
            font,
            layout,
        }
    }

    pub fn layout(&self) -> PillLayout {
        self.layout
    }

    pub fn mic_button_contains(&self, point: SurfacePoint) -> bool {
        self.layout
            .mic_button
            .is_some_and(|mic| mic.contains(point))
    }

    pub fn paint(
        &self,
        frame: &IndicatorFrame,
        spinner_turns: f32,
        scale: u32,
    ) -> Option<PaintedPill> {
        let logical = self.layout.size;
        let size = PixelSize {
            width: logical.width * scale,
            height: logical.height * scale,
        };
        let scale = scale as f32;
        let mut pixmap = Pixmap::new(size.width, size.height)?;
        pixmap.fill(Color::TRANSPARENT);

        let corner = self.popup.corner_radius_px as f32 * scale;
        paint_rounded_rect(
            &mut pixmap,
            size.width as f32,
            size.height as f32,
            corner,
            rgba_colour(self.popup.background_colour),
        );

        let style = self.popup.phase_styles.style_for(frame.phase);
        self.paint_status(&mut pixmap, frame, spinner_turns, scale, style.colour);
        if let Some(mic) = self.layout.mic_button {
            paint_mic(&mut pixmap, &mic, scale, MIC_COLOUR);
        }
        if let (Some(label), Some(font)) = (self.layout.label, self.font.as_ref()) {
            draw_label(
                &mut pixmap,
                font,
                &style.label_text,
                &label,
                scale,
                self.popup.phase_label.font_size_px as f32,
                style.colour,
            );
        }

        Some(PaintedPill {
            size,
            rgba: pixmap.take(),
        })
    }

    fn paint_status(
        &self,
        pixmap: &mut Pixmap,
        frame: &IndicatorFrame,
        spinner_turns: f32,
        scale: f32,
        colour: Rgba,
    ) {
        let status = self.layout.status;
        let meter = &self.popup.level_meter;
        if frame.phase.shows_level_bars() && meter.show_level_meter {
            paint_level_bars(
                pixmap,
                &frame.level_bar_heights,
                &status,
                meter,
                scale,
                colour,
            );
        } else if frame.phase.shows_spinner() {
            paint_spinner(pixmap, &status, scale, spinner_turns, colour);
        } else {
            paint_idle_dot(pixmap, &status, scale, colour);
        }
    }
}

fn widest_label_width(font: &FontArc, popup: &PopupConfig) -> f32 {
    let size_px = popup.phase_label.font_size_px as f32;
    IndicatorPhase::ALL
        .iter()
        .map(|phase| {
            let text = popup.phase_styles.style_for(*phase).label_text;
            text_width(font, &text, size_px)
        })
        .fold(0.0_f32, f32::max)
}

fn text_width(font: &FontArc, text: &str, size_px: f32) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size_px));
    text.chars()
        .map(|character| scaled.h_advance(scaled.glyph_id(character)))
        .sum()
}

fn rgba_colour(colour: Rgba) -> Color {
    Color::from_rgba8(colour[0], colour[1], colour[2], colour[3])
}

fn paint_rounded_rect(pixmap: &mut Pixmap, width: f32, height: f32, corner: f32, color: Color) {
    let Some(rect) = Rect::from_xywh(0.0, 0.0, width, height) else {
        return;
    };
    if let Some(path) = rounded_rect_path(rect, corner) {
        paint_fill(pixmap, color, &path);
    }
}

fn rounded_rect_path(rect: Rect, corner_radius: f32) -> Option<tiny_skia::Path> {
    let radius = corner_radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let (left, top, right, bottom) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    let mut path = PathBuilder::new();
    path.move_to(left + radius, top);
    path.line_to(right - radius, top);
    path.quad_to(right, top, right, top + radius);
    path.line_to(right, bottom - radius);
    path.quad_to(right, bottom, right - radius, bottom);
    path.line_to(left + radius, bottom);
    path.quad_to(left, bottom, left, bottom - radius);
    path.line_to(left, top + radius);
    path.quad_to(left, top, left + radius, top);
    path.close();
    path.finish()
}

fn paint_fill(pixmap: &mut Pixmap, color: Color, path: &tiny_skia::Path) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn paint_level_bars(
    pixmap: &mut Pixmap,
    levels: &[f32],
    rect: &RectF,
    meter: &LevelMeterConfig,
    scale: f32,
    colour: Rgba,
) {
    let mut paint = Paint::default();
    paint.set_color(rgba_colour(colour));
    paint.anti_alias = true;
    let bar_width = meter.bar_width_px as f32;
    let stride = bar_width + meter.spacing_between_bars_px as f32;
    let track_height = rect.height * scale;
    let track_top = rect.y * scale;
    levels
        .iter()
        .copied()
        .chain(std::iter::repeat(0.0))
        .take(meter.bar_count)
        .enumerate()
        .for_each(|(index, level)| {
            let bar_height = (track_height * level.clamp(0.0, 1.0)).max(1.0);
            let x = (rect.x + index as f32 * stride) * scale;
            let y = track_top + (track_height - bar_height) / 2.0;
            if let Some(bar) = Rect::from_xywh(x, y, bar_width * scale, bar_height) {
                pixmap.fill_rect(bar, &paint, Transform::identity(), None);
            }
        });
}

fn paint_spinner(pixmap: &mut Pixmap, rect: &RectF, scale: f32, turns: f32, colour: Rgba) {
    let centre_x = (rect.x + rect.width / 2.0) * scale;
    let centre_y = (rect.y + rect.height / 2.0) * scale;
    let orbit = rect.height * 0.32 * scale;
    let active = (turns * SPINNER_DOTS as f32).floor() as usize % SPINNER_DOTS;
    (0..SPINNER_DOTS).for_each(|index| {
        let angle = index as f32 / SPINNER_DOTS as f32 * TAU;
        let is_active = index == active;
        let dot_colour = match is_active {
            true => rgba_colour(colour),
            false => rgba_colour(INACTIVE_SPINNER_COLOUR),
        };
        let dot_radius = match is_active {
            true => rect.height * 0.2 * scale,
            false => rect.height * 0.11 * scale,
        };
        paint_dot(
            pixmap,
            centre_x + orbit * angle.cos(),
            centre_y + orbit * angle.sin(),
            dot_radius,
            dot_colour,
        );
    });
}

fn paint_idle_dot(pixmap: &mut Pixmap, rect: &RectF, scale: f32, colour: Rgba) {
    let centre_x = (rect.x + rect.width / 2.0) * scale;
    let centre_y = (rect.y + rect.height / 2.0) * scale;
    paint_dot(
        pixmap,
        centre_x,
        centre_y,
        rect.height * 0.25 * scale,
        rgba_colour(colour),
    );
}

fn paint_dot(pixmap: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: Color) {
    let mut path = PathBuilder::new();
    path.push_circle(cx, cy, radius);
    if let Some(path) = path.finish() {
        paint_fill(pixmap, color, &path);
    }
}

fn paint_mic(pixmap: &mut Pixmap, rect: &RectF, scale: f32, colour: Rgba) {
    let centre_x = (rect.x + rect.width / 2.0) * scale;
    let top = rect.y * scale;
    let height = rect.height * scale;
    let stroke = (height * 0.16).max(1.0);
    let mut paint = Paint::default();
    paint.set_color(rgba_colour(colour));
    paint.anti_alias = true;
    if let Some(capsule) = Rect::from_xywh(
        centre_x - stroke / 2.0,
        top + height * 0.12,
        stroke,
        height * 0.52,
    ) {
        if let Some(path) = rounded_rect_path(capsule, stroke) {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    let mut stem_dot = PathBuilder::new();
    stem_dot.push_circle(centre_x, top + height * 0.70, stroke * 1.2);
    if let Some(path) = stem_dot.finish() {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn draw_label(
    pixmap: &mut Pixmap,
    font: &FontArc,
    text: &str,
    rect: &RectF,
    scale: f32,
    font_size_px: f32,
    colour: Rgba,
) {
    let px = font_size_px * scale;
    let scaled = font.as_scaled(PxScale::from(px));
    let ids: Vec<GlyphId> = text.chars().map(|c| scaled.glyph_id(c)).collect();
    let text_width: f32 = ids.iter().map(|id| scaled.h_advance(*id)).sum();
    let start_x = (rect.x * scale) + ((rect.width * scale) - text_width).max(0.0) / 2.0;
    let baseline_y = (rect.y + rect.height * 0.68) * scale;
    ids.iter().fold(start_x, |pen_x, id| {
        let glyph = id.with_scale_and_position(px, point(pen_x, baseline_y));
        if let Some(outlined) = scaled.outline_glyph(glyph) {
            let glyph_bounds = outlined.px_bounds();
            outlined.draw(|x, y, coverage| {
                let pixel_x = glyph_bounds.min.x as i64 + i64::from(x);
                let pixel_y = glyph_bounds.min.y as i64 + i64::from(y);
                if let (Ok(pixel_x), Ok(pixel_y)) = (u32::try_from(pixel_x), u32::try_from(pixel_y))
                {
                    blend_pixel(pixmap, pixel_x, pixel_y, colour, coverage);
                }
            });
        }
        pen_x + scaled.h_advance(*id)
    });
}

fn blend_pixel(pixmap: &mut Pixmap, x: u32, y: u32, rgba: Rgba, coverage: f32) {
    if coverage <= 0.0 {
        return;
    }
    let (w, h) = (pixmap.width(), pixmap.height());
    if x >= w || y >= h {
        return;
    }
    let src_a = (rgba[3] as f32 / 255.0) * coverage.clamp(0.0, 1.0);
    let idx = ((y * w + x) * 4) as usize;
    let data = pixmap.data_mut();
    let dr = data[idx] as f32 / 255.0;
    let dg = data[idx + 1] as f32 / 255.0;
    let db = data[idx + 2] as f32 / 255.0;
    let da = data[idx + 3] as f32 / 255.0;
    let out_a = src_a + da * (1.0 - src_a);
    let src = |c: u8| c as f32 / 255.0;
    data[idx] = ((src(rgba[0]) * src_a + dr * (1.0 - src_a)) * 255.0).round() as u8;
    data[idx + 1] = ((src(rgba[1]) * src_a + dg * (1.0 - src_a)) * 255.0).round() as u8;
    data[idx + 2] = ((src(rgba[2]) * src_a + db * (1.0 - src_a)) * 255.0).round() as u8;
    data[idx + 3] = (out_a * 255.0).round() as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_LABEL_WIDTH: f32 = 60.0;

    fn painter_without_font() -> PillPainter {
        PillPainter::new(&PopupConfig::default(), None)
    }

    fn frame(phase: IndicatorPhase) -> IndicatorFrame {
        IndicatorFrame {
            phase,
            level_bar_heights: vec![0.5; 12],
        }
    }

    fn rects_of(layout: &PillLayout) -> Vec<RectF> {
        [layout.mic_button, Some(layout.status), layout.label]
            .into_iter()
            .flatten()
            .collect()
    }

    #[test]
    fn hex_parser_accepts_rgb_and_rgba_with_or_without_hash() {
        assert_eq!(parse_hex("#FF8000"), Some([0xff, 0x80, 0x00, 0xff]));
        assert_eq!(parse_hex("ff8000"), Some([0xff, 0x80, 0x00, 0xff]));
        assert_eq!(parse_hex("#ff800040"), Some([0xff, 0x80, 0x00, 0x40]));
        assert_eq!(parse_hex("nope"), None);
        assert_eq!(parse_hex("#ff80"), None);
        assert_eq!(parse_hex("#gg8000"), None);
    }

    #[test]
    fn default_layout_is_125_by_22_with_every_element() {
        let layout = PillLayout::from_popup_config(&PopupConfig::default(), DEFAULT_LABEL_WIDTH);
        assert_eq!(
            layout.size,
            PixelSize {
                width: 125,
                height: 22
            }
        );
        assert_eq!(rects_of(&layout).len(), 3);
    }

    #[test]
    fn regions_do_not_overlap_and_stay_inside_the_pill() {
        let layout = PillLayout::from_popup_config(&PopupConfig::default(), DEFAULT_LABEL_WIDTH);
        let rects = rects_of(&layout);
        assert!(rects
            .windows(2)
            .all(|pair| pair[0].x + pair[0].width <= pair[1].x));
        assert!(rects.iter().all(|rect| {
            rect.x >= 0.0
                && rect.y >= 0.0
                && rect.x + rect.width <= layout.size.width as f32
                && rect.y + rect.height <= layout.size.height as f32
        }));
    }

    #[test]
    fn disabled_elements_take_no_space() {
        let full = PillLayout::from_popup_config(&PopupConfig::default(), DEFAULT_LABEL_WIDTH);
        let mut without_mic = PopupConfig::default();
        without_mic.mic_button.show_mic_button = false;
        let mut without_label = PopupConfig::default();
        without_label.phase_label.show_phase_label = false;
        let mut without_meter = PopupConfig::default();
        without_meter.level_meter.show_level_meter = false;

        let mic_free = PillLayout::from_popup_config(&without_mic, DEFAULT_LABEL_WIDTH);
        let label_free = PillLayout::from_popup_config(&without_label, DEFAULT_LABEL_WIDTH);
        let meter_free = PillLayout::from_popup_config(&without_meter, DEFAULT_LABEL_WIDTH);

        assert!(mic_free.mic_button.is_none());
        assert_eq!(mic_free.size.width, full.size.width - 14 - 4);
        assert!(label_free.label.is_none());
        assert_eq!(label_free.size.width, full.size.width - 60 - 4);
        assert!(meter_free.size.width < full.size.width);
    }

    #[test]
    fn mic_hit_test_follows_the_layout() {
        let painter = painter_without_font();
        let mic = painter.layout().mic_button.expect("mic is on by default");
        let centre = SurfacePoint {
            x: f64::from(mic.x + mic.width / 2.0),
            y: f64::from(mic.y + mic.height / 2.0),
        };
        assert!(painter.mic_button_contains(centre));
        let status = painter.layout().status;
        let on_status = SurfacePoint {
            x: f64::from(status.x + status.width / 2.0),
            y: f64::from(status.y + status.height / 2.0),
        };
        assert!(!painter.mic_button_contains(on_status));

        let without_mic = PillPainter::new(
            &PopupConfig {
                mic_button: crate::ime_config::MicButtonConfig {
                    show_mic_button: false,
                    ..Default::default()
                },
                ..PopupConfig::default()
            },
            None,
        );
        assert!(!without_mic.mic_button_contains(centre));
    }

    #[test]
    fn every_phase_paints_at_one_and_two_times_scale() {
        let painter = painter_without_font();
        IndicatorPhase::ALL.iter().for_each(|phase| {
            [1_u32, 2].into_iter().for_each(|scale| {
                let pill = painter.paint(&frame(*phase), 0.25, scale).expect("paints");
                assert_eq!(
                    (pill.size.width, pill.size.height),
                    (
                        painter.layout().size.width * scale,
                        painter.layout().size.height * scale
                    )
                );
                assert_eq!(
                    pill.rgba.len(),
                    (pill.size.width * pill.size.height * 4) as usize
                );
                assert!(pill.rgba.iter().any(|byte| *byte != 0));
            });
        });
    }

    #[test]
    fn spinner_and_bars_draw_different_pixels() {
        let painter = painter_without_font();
        let bars = painter
            .paint(&frame(IndicatorPhase::Hearing), 0.0, 1)
            .expect("paints");
        let dot = painter
            .paint(&frame(IndicatorPhase::Ready), 0.0, 1)
            .expect("paints");
        assert_ne!(bars.rgba, dot.rgba);
    }
}
