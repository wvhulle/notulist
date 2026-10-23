use std::f32::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{point, Font, FontArc, GlyphId, PxScale, ScaleFont};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

use super::indicator_phase::{IndicatorFrame, IndicatorPhase};
use crate::ime_config::{LevelMeterConfig, PopupConfig, Rgba};

const SPINNER_DOTS: usize = 6;
const CAP_HEIGHT_PER_EM: f32 = 0.7;
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
    pub status: Option<RectF>,
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
    pub fn for_phase(popup: &PopupConfig, phase: IndicatorPhase, label_width: f32) -> Self {
        match phase.is_idle() {
            true => Self::idle_square(popup),
            false => Self::active_row(popup, phase, label_width),
        }
    }

    fn idle_square(popup: &PopupConfig) -> Self {
        let height = popup.pill_height_px as f32;
        let mic_size = (popup.mic_button.mic_button_size_px as f32).min(height);
        let mic_offset = (height - mic_size) / 2.0;
        Self {
            size: PixelSize {
                width: popup.pill_height_px,
                height: popup.pill_height_px,
            },
            mic_button: Some(RectF {
                x: mic_offset,
                y: mic_offset,
                width: mic_size,
                height: mic_size,
            }),
            status: None,
            label: None,
        }
    }

    fn active_row(popup: &PopupConfig, phase: IndicatorPhase, label_width: f32) -> Self {
        let height = popup.pill_height_px as f32;
        let padding = popup.inner_padding_px as f32;
        let inner_height = (height - 2.0 * padding).max(0.0);
        let mut row = ElementRow {
            next_x: padding,
            spacing: popup.spacing_between_elements_px as f32,
        };

        let mic_button = popup.mic_button.show_mic_button.then(|| {
            let size = (popup.mic_button.mic_button_size_px as f32).min(height);
            RectF {
                x: row.place(size),
                y: (height - size) / 2.0,
                width: size,
                height: size,
            }
        });

        let status = phase_has_status_body(popup, phase).then(|| {
            let width = status_body_width(popup, inner_height);
            RectF {
                x: row.place(width),
                y: padding,
                width,
                height: inner_height,
            }
        });

        let label = (popup.phase_label.show_phase_label && label_width > 0.0).then(|| RectF {
            x: row.place(label_width),
            y: padding,
            width: label_width,
            height: inner_height,
        });

        let content_end = [mic_button, status, label]
            .into_iter()
            .flatten()
            .map(|rect| rect.x + rect.width + padding)
            .fold(height, f32::max);
        Self {
            size: PixelSize {
                width: content_end.ceil() as u32,
                height: popup.pill_height_px,
            },
            mic_button,
            status,
            label,
        }
    }
}

fn phase_has_status_body(popup: &PopupConfig, phase: IndicatorPhase) -> bool {
    (phase.shows_level_bars() && popup.level_meter.show_level_meter) || phase.shows_spinner()
}

fn status_body_width(popup: &PopupConfig, inner_height: f32) -> f32 {
    match popup.level_meter.show_level_meter {
        true => meter_width(&popup.level_meter).max(inner_height),
        false => inner_height,
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
    label_width: f32,
}

impl PillPainter {
    pub fn new(popup: &PopupConfig, font: Option<FontArc>) -> Self {
        let label_width = font
            .as_ref()
            .map_or(0.0, |font| widest_label_width(font, popup));
        Self {
            popup: popup.clone(),
            font,
            label_width,
        }
    }

    pub fn layout_for(&self, phase: IndicatorPhase) -> PillLayout {
        PillLayout::for_phase(&self.popup, phase, self.label_width)
    }

    pub fn mic_button_contains(&self, phase: IndicatorPhase, point: SurfacePoint) -> bool {
        self.popup.mic_button.show_mic_button
            && self
                .layout_for(phase)
                .mic_button
                .is_some_and(|mic| mic.contains(point))
    }

    pub fn paint(
        &self,
        frame: &IndicatorFrame,
        spinner_turns: f32,
        scale: u32,
    ) -> Option<PaintedPill> {
        let layout = self.layout_for(frame.phase);
        let size = PixelSize {
            width: layout.size.width * scale,
            height: layout.size.height * scale,
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
        if let Some(status) = layout.status {
            self.paint_status(
                &mut pixmap,
                &status,
                frame,
                spinner_turns,
                scale,
                style.colour,
            );
        }
        if let Some(mic) = layout.mic_button {
            paint_mic_glyph(&mut pixmap, &mic, scale, style.colour);
        }
        if let (Some(label), Some(font)) = (layout.label, self.font.as_ref()) {
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
        status: &RectF,
        frame: &IndicatorFrame,
        spinner_turns: f32,
        scale: f32,
        colour: Rgba,
    ) {
        let meter = &self.popup.level_meter;
        if frame.phase.shows_level_bars() && meter.show_level_meter {
            let levels: &[f32] = match frame.phase.shows_flat_level_bars() {
                true => &[],
                false => &frame.level_bar_heights,
            };
            paint_level_bars(pixmap, levels, status, meter, scale, colour);
        } else if frame.phase.shows_spinner() {
            paint_spinner(pixmap, status, scale, spinner_turns, colour);
        }
    }
}

fn widest_label_width(font: &FontArc, popup: &PopupConfig) -> f32 {
    let size_px = popup.phase_label.font_size_px as f32;
    IndicatorPhase::ALL
        .iter()
        .filter(|phase| !phase.is_idle())
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
    let bar_width = meter.bar_width_px as f32;
    let stride = bar_width + meter.spacing_between_bars_px as f32;
    let track_left = rect.x + (rect.width - meter_width(meter)).max(0.0) / 2.0;
    let track_height = rect.height * scale;
    let track_top = rect.y * scale;
    let min_bar_height = (bar_width * scale).max(1.0);
    let colour = rgba_colour(colour);
    levels
        .iter()
        .copied()
        .chain(std::iter::repeat(0.0))
        .take(meter.bar_count)
        .enumerate()
        .for_each(|(index, level)| {
            let bar_height = (track_height * level.clamp(0.0, 1.0)).max(min_bar_height);
            let x = (track_left + index as f32 * stride) * scale;
            let y = track_top + (track_height - bar_height) / 2.0;
            let Some(bar) = Rect::from_xywh(x, y, bar_width * scale, bar_height) else {
                return;
            };
            if let Some(path) = rounded_rect_path(bar, bar_width * scale / 2.0) {
                paint_fill(pixmap, colour, &path);
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

fn paint_dot(pixmap: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: Color) {
    let mut path = PathBuilder::new();
    path.push_circle(cx, cy, radius);
    if let Some(path) = path.finish() {
        paint_fill(pixmap, color, &path);
    }
}

struct MicGlyphGeometry {
    stroke_width: f32,
    centre_x: f32,
    capsule: Rect,
    holder_top_y: f32,
    holder_centre_y: f32,
    holder_radius: f32,
    stand_bottom_y: f32,
    base_half_width: f32,
}

impl MicGlyphGeometry {
    fn fit_into(rect: &RectF, scale: f32) -> Option<Self> {
        let side = rect.width.min(rect.height) * scale;
        let left = rect.x * scale + (rect.width * scale - side) / 2.0;
        let top = rect.y * scale + (rect.height * scale - side) / 2.0;
        let stroke_width = (side * 0.1).round().max(1.0);
        let stroke_is_odd = stroke_width as u32 % 2 == 1;
        let centre_x = snap_to_stroke_grid(left + side / 2.0, stroke_is_odd);
        let capsule_width = width_with_parity(side * 0.38, stroke_is_odd);
        let capsule_top = top.round();
        let capsule_height = (side * 0.54).round();
        let capsule = Rect::from_xywh(
            centre_x - capsule_width / 2.0,
            capsule_top,
            capsule_width,
            capsule_height,
        )?;
        let holder_centre_y = capsule.bottom() - capsule_width / 2.0;
        let holder_radius = (side * 0.32).ceil();
        Some(Self {
            stroke_width,
            centre_x,
            capsule,
            holder_top_y: capsule_top + capsule_height * 0.45,
            holder_centre_y,
            holder_radius,
            stand_bottom_y: snap_to_stroke_grid(top + side * 0.94, stroke_is_odd),
            base_half_width: (holder_radius * 0.75).round(),
        })
    }

    fn holder_and_stand_path(&self) -> Option<tiny_skia::Path> {
        let left_x = self.centre_x - self.holder_radius;
        let right_x = self.centre_x + self.holder_radius;
        let bottom_y = self.holder_centre_y + self.holder_radius;
        let handle = self.holder_radius * QUARTER_CIRCLE_HANDLE;
        let mut path = PathBuilder::new();
        path.move_to(left_x, self.holder_top_y);
        path.line_to(left_x, self.holder_centre_y);
        path.cubic_to(
            left_x,
            self.holder_centre_y + handle,
            self.centre_x - handle,
            bottom_y,
            self.centre_x,
            bottom_y,
        );
        path.cubic_to(
            self.centre_x + handle,
            bottom_y,
            right_x,
            self.holder_centre_y + handle,
            right_x,
            self.holder_centre_y,
        );
        path.line_to(right_x, self.holder_top_y);
        path.move_to(self.centre_x, bottom_y);
        path.line_to(self.centre_x, self.stand_bottom_y);
        path.move_to(self.centre_x - self.base_half_width, self.stand_bottom_y);
        path.line_to(self.centre_x + self.base_half_width, self.stand_bottom_y);
        path.finish()
    }
}

const QUARTER_CIRCLE_HANDLE: f32 = 0.552_284_8;

fn snap_to_stroke_grid(coordinate: f32, stroke_is_odd: bool) -> f32 {
    match stroke_is_odd {
        true => coordinate.floor() + 0.5,
        false => coordinate.round(),
    }
}

fn width_with_parity(width: f32, odd: bool) -> f32 {
    let rounded = width.round().max(1.0);
    match (rounded as u32 % 2 == 1) == odd {
        true => rounded,
        false => rounded + 1.0,
    }
}

fn paint_mic_glyph(pixmap: &mut Pixmap, rect: &RectF, scale: f32, colour: Rgba) {
    let Some(geometry) = MicGlyphGeometry::fit_into(rect, scale) else {
        return;
    };
    let colour = rgba_colour(colour);
    if let Some(capsule) = rounded_rect_path(geometry.capsule, geometry.capsule.width() / 2.0) {
        paint_fill(pixmap, colour, &capsule);
    }
    let Some(holder) = geometry.holder_and_stand_path() else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(colour);
    paint.anti_alias = true;
    let stroke = Stroke {
        width: geometry.stroke_width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&holder, &paint, &stroke, Transform::identity(), None);
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
    let start_x = (rect.x * scale).round();
    let baseline_y = ((rect.y + rect.height / 2.0) * scale + px * CAP_HEIGHT_PER_EM / 2.0).round();
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
        [layout.mic_button, layout.status, layout.label]
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

    fn layout_of(phase: IndicatorPhase) -> PillLayout {
        PillLayout::for_phase(&PopupConfig::default(), phase, DEFAULT_LABEL_WIDTH)
    }

    fn centre_of(rect: RectF) -> SurfacePoint {
        SurfacePoint {
            x: f64::from(rect.x + rect.width / 2.0),
            y: f64::from(rect.y + rect.height / 2.0),
        }
    }

    #[test]
    fn idle_is_a_square_with_only_the_mic() {
        let layout = layout_of(IndicatorPhase::Ready);
        assert_eq!(
            layout.size,
            PixelSize {
                width: 22,
                height: 22
            }
        );
        assert!(layout.mic_button.is_some());
        assert!(layout.status.is_none());
        assert!(layout.label.is_none());
    }

    #[test]
    fn default_active_widths_depend_on_the_status_body() {
        assert_eq!(layout_of(IndicatorPhase::Hearing).size.width, 125);
        assert_eq!(layout_of(IndicatorPhase::Finishing).size.width, 125);
        assert_eq!(layout_of(IndicatorPhase::Committed).size.width, 86);
        assert_eq!(rects_of(&layout_of(IndicatorPhase::Hearing)).len(), 3);
        assert_eq!(rects_of(&layout_of(IndicatorPhase::Failed)).len(), 2);
    }

    #[test]
    fn idle_is_narrower_than_every_active_phase() {
        let idle_width = layout_of(IndicatorPhase::Ready).size.width;
        assert!(IndicatorPhase::ALL
            .iter()
            .filter(|phase| !phase.is_idle())
            .all(|phase| layout_of(*phase).size.width > idle_width));
    }

    #[test]
    fn phases_without_meter_or_spinner_show_only_mic_and_label() {
        [
            IndicatorPhase::Committed,
            IndicatorPhase::Cancelled,
            IndicatorPhase::Failed,
            IndicatorPhase::Offline,
            IndicatorPhase::Password,
        ]
        .into_iter()
        .map(layout_of)
        .for_each(|layout| {
            assert!(layout.status.is_none());
            assert!(layout.mic_button.is_some());
            assert!(layout.label.is_some());
        });
    }

    #[test]
    fn regions_do_not_overlap_and_stay_inside_the_pill() {
        IndicatorPhase::ALL
            .into_iter()
            .map(layout_of)
            .for_each(|layout| {
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
            });
    }

    #[test]
    fn disabled_elements_take_no_space() {
        let full = layout_of(IndicatorPhase::Hearing);
        let mut without_mic = PopupConfig::default();
        without_mic.mic_button.show_mic_button = false;
        let mut without_label = PopupConfig::default();
        without_label.phase_label.show_phase_label = false;
        let mut without_meter = PopupConfig::default();
        without_meter.level_meter.show_level_meter = false;

        let hearing_with = |popup: &PopupConfig| {
            PillLayout::for_phase(popup, IndicatorPhase::Hearing, DEFAULT_LABEL_WIDTH)
        };
        let mic_free = hearing_with(&without_mic);
        let label_free = hearing_with(&without_label);
        let meter_free = hearing_with(&without_meter);

        assert!(mic_free.mic_button.is_none());
        assert_eq!(mic_free.size.width, full.size.width - 14 - 4);
        assert!(label_free.label.is_none());
        assert_eq!(label_free.size.width, full.size.width - 60 - 4);
        assert!(meter_free.status.is_none());
        assert!(meter_free.size.width < full.size.width);
    }

    #[test]
    fn mic_hit_test_follows_the_layout() {
        let painter = painter_without_font();
        [IndicatorPhase::Ready, IndicatorPhase::Hearing]
            .into_iter()
            .for_each(|phase| {
                let mic = painter.layout_for(phase).mic_button.expect("mic is on");
                assert!(painter.mic_button_contains(phase, centre_of(mic)));
            });

        let status = painter
            .layout_for(IndicatorPhase::Hearing)
            .status
            .expect("hearing has a meter");
        assert!(!painter.mic_button_contains(IndicatorPhase::Hearing, centre_of(status)));
        assert!(!painter.mic_button_contains(IndicatorPhase::Ready, centre_of(status)));

        let idle_mic = painter
            .layout_for(IndicatorPhase::Ready)
            .mic_button
            .expect("idle shows the mic");
        let hearing_mic = painter
            .layout_for(IndicatorPhase::Hearing)
            .mic_button
            .expect("mic is on");

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
        assert!(!without_mic.mic_button_contains(IndicatorPhase::Ready, centre_of(idle_mic)));
        assert!(!without_mic.mic_button_contains(IndicatorPhase::Hearing, centre_of(hearing_mic)));
    }

    #[test]
    fn every_phase_paints_at_one_and_two_times_scale() {
        let painter = painter_without_font();
        IndicatorPhase::ALL.iter().for_each(|phase| {
            [1_u32, 2].into_iter().for_each(|scale| {
                let pill = painter.paint(&frame(*phase), 0.25, scale).expect("paints");
                let logical = painter.layout_for(*phase).size;
                assert_eq!(
                    (pill.size.width, pill.size.height),
                    (logical.width * scale, logical.height * scale)
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
        let spinner = painter
            .paint(&frame(IndicatorPhase::Finishing), 0.0, 1)
            .expect("paints");
        assert_eq!(bars.size, spinner.size);
        assert_ne!(bars.rgba, spinner.rgba);
    }

    #[test]
    fn waiting_bars_ignore_the_levels() {
        let painter = painter_without_font();
        let loud = painter
            .paint(&frame(IndicatorPhase::Waiting), 0.0, 1)
            .expect("paints");
        let silent = painter
            .paint(
                &IndicatorFrame {
                    phase: IndicatorPhase::Waiting,
                    level_bar_heights: Vec::new(),
                },
                0.0,
                1,
            )
            .expect("paints");
        assert_eq!(loud.rgba, silent.rgba);
    }

    #[test]
    fn mic_glyph_stays_inside_its_slot() {
        let slot = RectF {
            x: 4.0,
            y: 4.0,
            width: 14.0,
            height: 14.0,
        };
        [1.0_f32, 2.0].into_iter().for_each(|scale| {
            let geometry = MicGlyphGeometry::fit_into(&slot, scale).expect("fits");
            let half_stroke = geometry.stroke_width / 2.0;
            let holder_inner = geometry.holder_radius - half_stroke;
            assert!(geometry.capsule.width() / 2.0 + 1.5 <= holder_inner);
            assert!(geometry.capsule.top() >= slot.y * scale);
            assert!(geometry.stand_bottom_y + half_stroke <= (slot.y + slot.height) * scale + 0.01);
            assert!(geometry.centre_x - geometry.holder_radius - half_stroke >= slot.x * scale);
        });
    }
}
