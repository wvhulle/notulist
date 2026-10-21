//! Rasterises the indicator pill with `tiny-skia` and `ab_glyph`.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use ab_glyph::{point, Font, FontArc, GlyphId, PxScale};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

use super::dictation_indicator_state::IndicatorVisual;
use crate::config::InputMethodIndicatorConfig;

const PILL_WIDTH: f32 = 156.0;
const PILL_HEIGHT: f32 = 40.0;
const PADDING: f32 = 8.0;
const MIC_SIZE: f32 = 24.0;
const LABEL_WIDTH: f32 = 82.0;
const CORNER_RADIUS: f32 = 10.0;
const WAVEFORM_BARS: usize = 12;
const WAVEFORM_BAR_WIDTH: f32 = 3.0;
const SPINNER_DOTS: usize = 6;
const FONT_SIZE: f32 = 12.0;

#[derive(Debug, Clone, Copy)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct PillLayout {
    pub size: (u32, u32),
    pub mic_rect: RectF,
    pub waveform_rect: RectF,
    pub label_rect: RectF,
}

pub fn layout() -> PillLayout {
    let height = PILL_HEIGHT;
    let mic_x = PADDING;
    let mic_y = (height - MIC_SIZE) / 2.0;
    let waveform_x = PADDING + MIC_SIZE + PADDING;
    let label_x = PILL_WIDTH - PADDING - LABEL_WIDTH;
    let inner_height = height - 2.0 * PADDING;
    PillLayout {
        size: (PILL_WIDTH as u32, height as u32),
        mic_rect: RectF {
            x: mic_x,
            y: mic_y,
            width: MIC_SIZE,
            height: MIC_SIZE,
        },
        waveform_rect: RectF {
            x: waveform_x,
            y: PADDING,
            width: (label_x - PADDING - waveform_x).max(0.0),
            height: inner_height,
        },
        label_rect: RectF {
            x: label_x,
            y: PADDING,
            width: LABEL_WIDTH,
            height: inner_height,
        },
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: [u8; 4],
    pub foreground: [u8; 4],
    pub accent: [u8; 4],
    pub warning: [u8; 4],
}

impl Palette {
    pub fn from_config(config: &InputMethodIndicatorConfig) -> Self {
        Self {
            background: config
                .background
                .as_deref()
                .and_then(parse_hex)
                .unwrap_or([0x1e, 0x1e, 0x22, 0xe6]),
            foreground: config
                .foreground
                .as_deref()
                .and_then(parse_hex)
                .unwrap_or([0xa0, 0xa0, 0xa8, 0xff]),
            accent: config
                .accent
                .as_deref()
                .and_then(parse_hex)
                .unwrap_or([0x3b, 0x82, 0xf6, 0xff]),
            warning: config
                .warning
                .as_deref()
                .and_then(parse_hex)
                .unwrap_or([0xf5, 0x9e, 0x0b, 0xff]),
        }
    }
}

pub fn parse_hex(input: &str) -> Option<[u8; 4]> {
    let s = input.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r, g, b, 0xff])
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

pub fn resolve_font(override_path: Option<&str>) -> Option<FontArc> {
    if let Some(path) = override_path {
        return load_font(PathBuf::from(path));
    }
    static DEFAULT: OnceLock<Option<FontArc>> = OnceLock::new();
    DEFAULT
        .get_or_init(|| default_font_path().and_then(load_font))
        .clone()
}

/// Paint the pill at a physical pixel scale and return `(width, height, rgba)`.
///
/// `levels` are waveform peaks in 0..=1; `phase` is the spinner angle in turns.
pub fn paint(
    visual: IndicatorVisual,
    levels: &[f32],
    phase: f32,
    scale: u32,
    palette: &Palette,
    font: Option<&FontArc>,
) -> Option<(u32, u32, Vec<u8>)> {
    let PillLayout { size, mic_rect, waveform_rect, label_rect } = layout();
    let (logical_w, logical_h) = size;
    let (pixmap_w, pixmap_h) = (logical_w * scale, logical_h * scale);
    let scale = scale as f32;
    let mut pixmap = Pixmap::new(pixmap_w, pixmap_h)?;
    pixmap.fill(Color::TRANSPARENT);

    let background = Color::from_rgba8(
        palette.background[0],
        palette.background[1],
        palette.background[2],
        palette.background[3],
    );
    paint_rounded_rect(
        &mut pixmap,
        pixmap_w as f32,
        pixmap_h as f32,
        CORNER_RADIUS * scale,
        background,
    );

    let accent = palette.accent;
    let (label, body) = match visual {
        IndicatorVisual::Off | IndicatorVisual::Idle => {
            ("Passive", palette.foreground)
        }
        IndicatorVisual::Listening => ("Listening", accent),
        IndicatorVisual::Processing => ("Processing", palette.warning),
    };

    let body_fill = Color::from_rgba8(body[0], body[1], body[2], body[3]);
    match visual {
        IndicatorVisual::Listening => paint_waveform(
            &mut pixmap,
            levels,
            &waveform_rect,
            scale,
            body_fill,
        ),
        IndicatorVisual::Processing => paint_spinner(&mut pixmap, &waveform_rect, scale, phase, body_fill),
        _ => paint_idle_dot(&mut pixmap, &waveform_rect, scale, body_fill),
    }

    paint_mic(&mut pixmap, &mic_rect, scale, palette.foreground);
    if let Some(font) = font {
        draw_label(&mut pixmap, font, label, &label_rect, scale, body);
    }

    Some((pixmap_w, pixmap_h, pixmap.take()))
}

fn paint_rounded_rect(pixmap: &mut Pixmap, width: f32, height: f32, corner: f32, color: Color) {
    let Some(rect) = Rect::from_xywh(0.0, 0.0, width, height) else {
        return;
    };
    let mut path = PathBuilder::new();
    path.push_rounded_rect(rect, corner, corner);
    let Some(path) = path.finish() else {
        return;
    };
    paint_fill(pixmap, color, &path);
}

fn paint_fill(pixmap: &mut Pixmap, color: Color, path: &tiny_skia::Path) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn paint_waveform(
    pixmap: &mut Pixmap,
    levels: &[f32],
    rect: &RectF,
    scale: f32,
    color: Color,
) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    let slot = scale * (WAVEFORM_BAR_WIDTH + 2.0);
    let bars = WAVEFORM_BARS.min(levels.len().max(1));
    let origin_x = (rect.x + (rect.width - slot * bars as f32) / 2.0) * scale;
    let origin_y = rect.y * scale;
    let height = rect.height * scale;
    for i in 0..bars {
        let level = levels.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        let bar_h = (height * level).max(1.0);
        let x = origin_x + i as f32 * slot;
        let y = origin_y + (height - bar_h) / 2.0;
        let bar_rect = Rect::from_xywh(x, y, WAVEFORM_BAR_WIDTH * scale, bar_h).unwrap();
        pixmap.fill_rect(bar_rect, &paint, Transform::identity(), None);
    }
}

fn paint_spinner(pixmap: &mut Pixmap, rect: &RectF, scale: f32, phase: f32, color: Color) {
    let cx = (rect.x + rect.width / 2.0) * scale;
    let cy = (rect.y + rect.height / 2.0) * scale;
    let radius = (rect.height * 0.32) * scale;
    let active = (phase * SPINNER_DOTS as f32).floor() as usize % SPINNER_DOTS;
    for i in 0..SPINNER_DOTS {
        let angle = i as f32 / SPINNER_DOTS as f32 * std::f32::consts::TAU;
        let (dx, dy) = (radius * angle.cos(), radius * angle.sin());
        let dot_r = if i == active { 3.0 * scale } else { 1.6 * scale };
        paint_dot(pixmap, cx + dx, cy + dy, dot_r, if i == active { color } else { Color::from_rgba8(0x66, 0x66, 0x6e, 0xff) });
    }
}

fn paint_idle_dot(pixmap: &mut Pixmap, rect: &RectF, scale: f32, color: Color) {
    let cx = (rect.x + rect.width / 2.0) * scale;
    let cy = (rect.y + rect.height / 2.0) * scale;
    paint_dot(pixmap, cx, cy, 3.5 * scale, color);
}

fn paint_dot(pixmap: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: Color) {
    let mut path = PathBuilder::new();
    path.push_circle(cx, cy, radius);
    let Some(path) = path.finish() else {
        return;
    };
    paint_fill(pixmap, color, &path);
}

fn paint_mic(pixmap: &mut Pixmap, rect: &RectF, scale: f32, color: [u8; 4]) {
    let cx = (rect.x + rect.width / 2.0) * scale;
    let cy = rect.y * scale;
    let h = rect.height * scale;
    let stroke = 3.0 * scale;
    let mic_color = Color::from_rgba8(color[0], color[1], color[2], color[3]);
    let mut paint = Paint::default();
    paint.set_color(mic_color);
    paint.anti_alias = true;
    let capsule = Rect::from_xywh(cx - stroke / 2.0, cy + h * 0.12, stroke, h * 0.52).unwrap();
    let mut path = PathBuilder::new();
    path.push_rounded_rect(capsule, stroke, stroke);
    if let Some(path) = path.finish() {
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    let mut dot = PathBuilder::new();
    dot.push_circle(cx, cy + h * 0.70, stroke * 1.2);
    if let Some(path) = dot.finish() {
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn draw_label(pixmap: &mut Pixmap, font: &FontArc, text: &str, rect: &RectF, scale: f32, color: [u8; 4]) {
    let px = FONT_SIZE * scale;
    let scaled = font.as_scaled(PxScale::from(px));
    let ids: Vec<GlyphId> = text.chars().map(|c| scaled.glyph_id(c)).collect();
    let text_width: f32 = ids.iter().map(|id| scaled.h_advance(*id)).sum();
    let mut pen_x = (rect.x * scale) + ((rect.width * scale) - text_width).max(0.0) / 2.0;
    let baseline_y = (rect.y + rect.height * 0.68) * scale;
    for id in ids {
        let glyph = id.with_scale_and_position(px, point(pen_x, baseline_y));
        if let Some(outlined) = scaled.outline_glyph(glyph) {
            outlined.draw(|x, y, coverage| {
                blend_pixel(pixmap, x, y, color, coverage);
            });
        }
        pen_x += scaled.h_advance(id);
    }
}

fn blend_pixel(pixmap: &mut Pixmap, x: u32, y: u32, rgba: [u8; 4], coverage: f32) {
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

    #[test]
    fn hex_parser_accepts_hash_and_bare() {
        assert_eq!(parse_hex("#FF8000"), Some([0xff, 0x80, 0x00, 0xff]));
        assert_eq!(parse_hex("ff8000"), Some([0xff, 0x80, 0x00, 0xff]));
        assert_eq!(parse_hex("nope"), None);
    }

    #[test]
    fn layout_keeps_pill_regions_apart() {
        let PillLayout { size, mic_rect, waveform_rect, label_rect } = layout();
        assert_eq!(size.0 as f32, PILL_WIDTH);
        assert_eq!(size.1 as f32, PILL_HEIGHT);
        assert!(mic_rect.x >= 0.0 && mic_rect.x + mic_rect.width <= size.0 as f32);
        assert!(waveform_rect.x > mic_rect.x + mic_rect.width);
        assert!(label_rect.x > waveform_rect.x + waveform_rect.width);
        assert!(label_rect.x + label_rect.width <= size.0 as f32);
    }

    #[test]
    fn idle_paints_a_passive_pill() {
        let palette = Palette::from_config(&InputMethodIndicatorConfig::default());
        let (w, h, data) = paint(IndicatorVisual::Idle, &[], 0.0, 1, &palette, None).unwrap();
        assert_eq!((w, h), layout().size);
        assert_eq!(data.len(), (w * h * 4) as usize);
        assert_ne!(data, vec![0u8; data.len()]);
    }
}