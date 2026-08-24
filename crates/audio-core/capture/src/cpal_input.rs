//! Cross-platform physical input device capture (mic/line-in), via `cpal`.
//! Also used as the receiver's playback device driver (see `apps/receiver`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::Producer;

use crate::{AudioFormat, AudioSource, AudioStream, CaptureError, ThreadCaptureHandle};

pub struct CpalInputSource {
    device_name: Option<String>,
}

impl CpalInputSource {
    pub fn default_device() -> Self {
        Self { device_name: None }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self { device_name: Some(name.into()) }
    }
}

/// Names of available input devices (mic/line-in, and on Linux — via
/// PulseAudio/PipeWire — monitor sources, which is how a loopback-style
/// capture is done there since `cpal` has no WASAPI-style loopback API).
/// Intended for populating a device picker; best-effort (empty on error).
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

fn resolve_input_device(name: Option<&str>) -> Result<cpal::Device, CaptureError> {
    let host = cpal::default_host();
    match name {
        Some(name) => host
            .input_devices()
            .map_err(|e| CaptureError::Device(e.to_string()))?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .ok_or_else(|| CaptureError::Device(format!("input device '{name}' not found"))),
        None => host.default_input_device().ok_or(CaptureError::NoDefaultDevice),
    }
}

impl AudioSource for CpalInputSource {
    fn name(&self) -> String {
        self.device_name.clone().unwrap_or_else(|| "default input device".to_string())
    }

    fn start(self: Box<Self>, mut producer: Producer<f32>) -> Result<(Box<dyn AudioStream>, AudioFormat), CaptureError> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<AudioFormat, CaptureError>>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let device_name = self.device_name.clone();

        let join = thread::Builder::new()
            .name("audio-capture-input".into())
            .spawn(move || {
                let setup = (|| -> Result<(cpal::Stream, AudioFormat), CaptureError> {
                    let device = resolve_input_device(device_name.as_deref())?;
                    let supported = device.default_input_config().map_err(|e| CaptureError::UnsupportedConfig(e.to_string()))?;
                    if supported.sample_format() != cpal::SampleFormat::F32 {
                        return Err(CaptureError::UnsupportedConfig(format!(
                            "device default format is {:?}, only F32 is supported",
                            supported.sample_format()
                        )));
                    }
                    let config: cpal::StreamConfig = supported.into();
                    let format = AudioFormat { sample_rate: config.sample_rate.0, channels: config.channels as u8 };

                    let stream = device
                        .build_input_stream(
                            &config,
                            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                                for &sample in data {
                                    if producer.push(sample).is_err() {
                                        break; // ring full (downstream stalled): drop the rest of this callback
                                    }
                                }
                            },
                            |err| tracing::error!("cpal input stream error: {err}"),
                            None,
                        )
                        .map_err(|e| CaptureError::StreamStart(e.to_string()))?;
                    stream.play().map_err(|e| CaptureError::StreamStart(e.to_string()))?;
                    Ok((stream, format))
                })();

                match setup {
                    Ok((stream, format)) => {
                        let _ = ready_tx.send(Ok(format));
                        while !stop_thread.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_millis(20));
                        }
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .expect("failed to spawn capture thread");

        let format = ready_rx
            .recv()
            .map_err(|_| CaptureError::StreamStart("capture thread exited before initializing".into()))??;

        Ok((Box::new(ThreadCaptureHandle::new(stop, join)), format))
    }
}
