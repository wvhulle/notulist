use std::error::Error;
use std::time::Duration;

use notulist_ime::ime_config::ImeConfig;
use notulist_ime::input_method::indicator_phase::{IndicatorFrame, IndicatorPhase};
use notulist_ime::input_method::wayland_input_method_connection::InputMethodConnection;

const PHASE_DWELL: Duration = Duration::from_secs(2);
const BAR_TICK: Duration = Duration::from_millis(33);

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt().init();
    let config = ImeConfig::load(None)?;
    let bar_count = config.popup.level_meter.bar_count;
    let (mic_clicks_tx, mut mic_clicks_rx) = tokio::sync::mpsc::unbounded_channel();
    let connection = InputMethodConnection::spawn(config.popup, mic_clicks_tx);
    println!("Focus a text field; the indicator cycles through every phase. Ctrl+C to quit.");

    let mut phase_index = 0_usize;
    loop {
        let phase = IndicatorPhase::ALL[phase_index % IndicatorPhase::ALL.len()];
        phase_index += 1;
        println!("Showing {phase:?}");
        let mut step = 0_u32;
        connection.show_indicator_frame(synthetic_frame(phase, bar_count, step));
        let dwell = tokio::time::sleep(PHASE_DWELL);
        tokio::pin!(dwell);
        let mut bar_ticks = tokio::time::interval(BAR_TICK);
        loop {
            tokio::select! {
                _ = &mut dwell => break,
                Some(()) = mic_clicks_rx.recv() => println!("Mic button clicked"),
                _ = bar_ticks.tick() => {
                    step = step.wrapping_add(1);
                    connection.show_indicator_frame(synthetic_frame(phase, bar_count, step));
                }
            }
        }
    }
}

fn synthetic_frame(phase: IndicatorPhase, bar_count: usize, step: u32) -> IndicatorFrame {
    let level_bar_heights = (0..bar_count)
        .map(|index| {
            let wave = f64::from(step) * 0.3 + index as f64 * 0.5;
            ((wave.sin() + 1.0) / 2.0) as f32
        })
        .collect();
    IndicatorFrame {
        phase,
        level_bar_heights,
    }
}
