use std::sync::{Arc, Mutex};
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::{mpsc, oneshot};

use super::resampler::StreamResampler;
use super::{CapturedAudio, StopCapture};
use crate::config::MicrophoneConfig;
use crate::error::MicrophoneError;

const CHUNK_QUEUE_DEPTH: usize = 64;

type SharedResampler = Arc<Mutex<Option<StreamResampler>>>;

struct CaptureSetup {
    device: cpal::Device,
    device_name: String,
    supported: cpal::SupportedStreamConfig,
    target_rate_hz: u32,
    chunks_tx: mpsc::Sender<Vec<f32>>,
    disconnect_tx: oneshot::Sender<MicrophoneError>,
}

pub fn start_microphone(config: &MicrophoneConfig) -> Result<CapturedAudio, MicrophoneError> {
    let host = cpal::default_host();
    let device = find_input_device(&host, &config.device_name)?;
    let device_name = device.name().unwrap_or_default();
    tracing::info!(device = %device_name, "Using audio device");
    let supported = device.default_input_config()?;

    let (chunks_tx, chunks_rx) = mpsc::channel(CHUNK_QUEUE_DEPTH);
    let (stop_tx, stop_rx) = oneshot::channel();
    let (disconnect_tx, disconnect_rx) = oneshot::channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let setup = CaptureSetup {
        device,
        device_name,
        supported,
        target_rate_hz: config.capture_sample_rate_hz,
        chunks_tx,
        disconnect_tx,
    };
    thread::spawn(move || run_capture_thread(setup, stop_rx, ready_tx));
    ready_rx.recv()??;

    Ok(CapturedAudio {
        chunks: chunks_rx,
        stop: StopCapture::new(stop_tx),
        disconnect: disconnect_rx,
    })
}

fn run_capture_thread(
    setup: CaptureSetup,
    stop_rx: oneshot::Receiver<()>,
    ready_tx: std::sync::mpsc::SyncSender<Result<(), MicrophoneError>>,
) {
    let CaptureSetup {
        device,
        device_name,
        supported,
        target_rate_hz,
        chunks_tx,
        disconnect_tx,
    } = setup;
    let resampler: SharedResampler = Arc::new(Mutex::new(new_resampler(
        supported.sample_rate().0,
        target_rate_hz,
    )));
    let stream = match open_stream(
        &device,
        &device_name,
        &supported,
        Arc::clone(&resampler),
        chunks_tx.clone(),
        disconnect_tx,
    ) {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(device = %device_name, %error, "Could not open the microphone");
            let _ = ready_tx.send(Err(error));
            return;
        }
    };
    let _ = ready_tx.send(Ok(()));

    let _ = stop_rx.blocking_recv();
    drop(stream);
    let tail = flush_resampler(&resampler);
    if !tail.is_empty() {
        let _ = chunks_tx.blocking_send(tail);
    }
    tracing::debug!("Audio capture thread stopped");
}

fn new_resampler(source_rate_hz: u32, target_rate_hz: u32) -> Option<StreamResampler> {
    if source_rate_hz == target_rate_hz {
        return None;
    }
    match StreamResampler::new(source_rate_hz, target_rate_hz) {
        Ok(resampler) => Some(resampler),
        Err(error) => {
            tracing::error!(%error, "Capture will run without rate conversion");
            None
        }
    }
}

fn open_stream(
    device: &cpal::Device,
    device_name: &str,
    supported: &cpal::SupportedStreamConfig,
    resampler: SharedResampler,
    chunks_tx: mpsc::Sender<Vec<f32>>,
    disconnect_tx: oneshot::Sender<MicrophoneError>,
) -> Result<cpal::Stream, MicrophoneError> {
    let stream_config = cpal::StreamConfig {
        channels: supported.channels(),
        sample_rate: supported.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    };
    let channels = usize::from(supported.channels());
    let on_error = disconnect_on_error(device_name.to_string(), disconnect_tx);
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_input_stream::<f32>(
            device,
            &stream_config,
            channels,
            resampler,
            chunks_tx,
            on_error,
        )?,
        cpal::SampleFormat::I16 => build_input_stream::<i16>(
            device,
            &stream_config,
            channels,
            resampler,
            chunks_tx,
            on_error,
        )?,
        cpal::SampleFormat::U16 => build_input_stream::<u16>(
            device,
            &stream_config,
            channels,
            resampler,
            chunks_tx,
            on_error,
        )?,
        format => return Err(MicrophoneError::UnsupportedSampleFormat(format)),
    };
    stream.play()?;
    Ok(stream)
}

fn disconnect_on_error(
    device_name: String,
    disconnect_tx: oneshot::Sender<MicrophoneError>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    let mut disconnect_tx = Some(disconnect_tx);
    move |error| {
        tracing::error!(
            device = %device_name,
            %error,
            "Microphone stream failed; this recording is dead"
        );
        if let Some(tx) = disconnect_tx.take() {
            let _ = tx.send(MicrophoneError::Disconnected);
        }
    }
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    resampler: SharedResampler,
    chunks_tx: mpsc::Sender<Vec<f32>>,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, MicrophoneError>
where
    T: cpal::Sample + cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mono: Vec<f32> = data
                .chunks(channels)
                .map(|frame| {
                    let sum: f32 = frame
                        .iter()
                        .map(|&sample| <f32 as cpal::FromSample<T>>::from_sample_(sample))
                        .sum();
                    sum / channels as f32
                })
                .collect();
            let _ = chunks_tx.try_send(resample_chunk(&resampler, mono));
        },
        on_error,
        None,
    )?;
    Ok(stream)
}

fn resample_chunk(resampler: &SharedResampler, mono: Vec<f32>) -> Vec<f32> {
    let Ok(mut guard) = resampler.lock() else {
        return mono;
    };
    match guard.as_mut() {
        Some(resampler) => resampler.push(&mono),
        None => mono,
    }
}

fn flush_resampler(resampler: &SharedResampler) -> Vec<f32> {
    let Ok(mut guard) = resampler.lock() else {
        return Vec::new();
    };
    guard.as_mut().map_or_else(Vec::new, StreamResampler::flush)
}

fn find_input_device(
    host: &cpal::Host,
    requested_name: &str,
) -> Result<cpal::Device, MicrophoneError> {
    if requested_name == "default" {
        return host
            .default_input_device()
            .ok_or(MicrophoneError::NoDefaultDevice);
    }
    let devices: Vec<cpal::Device> = host.input_devices()?.collect();
    let names: Vec<String> = devices
        .iter()
        .map(|device| device.name().unwrap_or_default())
        .collect();
    let needle = requested_name.to_lowercase();
    let matched = names
        .iter()
        .position(|name| name == requested_name)
        .or_else(|| names.iter().position(|name| name.to_lowercase() == needle))
        .or_else(|| {
            names
                .iter()
                .position(|name| name.to_lowercase().contains(&needle))
        });
    let Some(index) = matched else {
        tracing::error!(requested = requested_name, available = ?names, "No input device matches the configured name");
        return Err(MicrophoneError::DeviceNotFound);
    };
    tracing::debug!(device = %names[index], "Matched input device");
    devices
        .into_iter()
        .nth(index)
        .ok_or(MicrophoneError::DeviceNotFound)
}
