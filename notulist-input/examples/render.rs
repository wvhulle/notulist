use std::error::Error;
use std::path::{Path, PathBuf};

use notulist_input::ime_config::ImeConfig;
use notulist_input::input_method::dictation_indicator_painter::{resolve_font, PillPainter};
use notulist_input::input_method::indicator_phase::{IndicatorFrame, IndicatorPhase};
use tiny_skia::{IntSize, Pixmap};

const PREVIEW_SCALES: [u32; 2] = [1, 2];
const SPEECH_LIKE_BAR_HEIGHTS: [f32; 12] = [
    0.05, 0.2, 0.55, 0.9, 0.7, 0.35, 0.6, 1.0, 0.8, 0.4, 0.15, 0.05,
];

fn main() -> Result<(), Box<dyn Error>> {
    let config = ImeConfig::load(None)?;
    let font = resolve_font(config.popup.font_file.as_deref());
    if font.is_none() {
        eprintln!(
            "No font found; labels are left out. Set [ime.popup] font_file or install fontconfig."
        );
    }
    let output_dir = preview_output_dir();
    std::fs::create_dir_all(&output_dir)?;
    let painter = PillPainter::new(&config.popup, font);
    IndicatorPhase::ALL.into_iter().try_for_each(|phase| {
        PREVIEW_SCALES
            .into_iter()
            .try_for_each(|scale| render_preview(&painter, &output_dir, phase, scale))
    })
}

fn render_preview(
    painter: &PillPainter,
    output_dir: &Path,
    phase: IndicatorPhase,
    scale: u32,
) -> Result<(), Box<dyn Error>> {
    let frame = IndicatorFrame {
        phase,
        level_bar_heights: SPEECH_LIKE_BAR_HEIGHTS.to_vec(),
    };
    let Some(pill) = painter.paint(&frame, 0.25, scale) else {
        return Err("painting failed".into());
    };
    let Some(size) = IntSize::from_wh(pill.size.width, pill.size.height) else {
        return Err("empty pill".into());
    };
    let Some(pixmap) = Pixmap::from_vec(pill.rgba, size) else {
        return Err("pixel buffer does not match the pill size".into());
    };
    let name = format!("{phase:?}").to_lowercase();
    let path = output_dir.join(format!("{name}@{scale}x.png"));
    pixmap.save_png(&path)?;
    println!("{}", path.display());
    Ok(())
}

fn preview_output_dir() -> PathBuf {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/indicator-preview")
        })
}
