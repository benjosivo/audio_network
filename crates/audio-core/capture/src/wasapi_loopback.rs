//! WASAPI loopback capture (Windows only): captures whatever is currently
//! playing on the default render (output) device, the way a virtual-audio-
//! cable style app would. `cpal` has no native loopback support, so this
//! talks to WASAPI directly via the `wasapi` crate.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use rtrb::Producer;
use wasapi::{Direction, SampleType, ShareMode};

use crate::{AudioFormat, AudioSource, AudioStream, CaptureError, ThreadCaptureHandle};

pub struct WasapiLoopbackSource;

impl WasapiLoopbackSource {
    /// Loops back the current default output ("render") device.
    pub fn default_device() -> Self {
        Self
    }
}

impl AudioSource for WasapiLoopbackSource {
    fn name(&self) -> String {
        "WASAPI loopback (default output device)".to_string()
    }

    fn start(self: Box<Self>, mut producer: Producer<f32>) -> Result<(Box<dyn AudioStream>, AudioFormat), CaptureError> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<AudioFormat, CaptureError>>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();

        let join = thread::Builder::new()
            .name("audio-capture-wasapi-loopback".into())
            .spawn(move || {
                let _ = wasapi::initialize_mta(); // COM apartment is per-thread; ignore "already initialized"

                let run = (|| -> Result<AudioFormat, CaptureError> {
                    let device = wasapi::get_default_device(&Direction::Render)
                        .map_err(|e| CaptureError::Device(e.to_string()))?;
                    let mut audio_client = device.get_iaudioclient().map_err(|e| CaptureError::Device(e.to_string()))?;

                    let desired_format = audio_client.get_mixformat().map_err(|e| CaptureError::Device(e.to_string()))?;
                    let sample_rate = desired_format.get_samplespersec();
                    let channels = desired_format.get_nchannels();
                    let blockalign = desired_format.get_blockalign();
                    match desired_format.get_subformat() {
                        Ok(SampleType::Float) => {}
                        Ok(other) => {
                            return Err(CaptureError::UnsupportedConfig(format!(
                                "default render device mix format is {other:?}, only Float is supported"
                            )));
                        }
                        Err(e) => return Err(CaptureError::UnsupportedConfig(format!("could not read mix format: {e}"))),
                    }

                    let (_default_period, min_period) =
                        audio_client.get_periods().map_err(|e| CaptureError::Device(e.to_string()))?;
                    audio_client
                        .initialize_client(&desired_format, min_period, &Direction::Capture, &ShareMode::Shared, true)
                        .map_err(|e| CaptureError::StreamStart(e.to_string()))?;

                    let event_handle = audio_client.set_get_eventhandle().map_err(|e| CaptureError::StreamStart(e.to_string()))?;
                    let capture_client =
                        audio_client.get_audiocaptureclient().map_err(|e| CaptureError::StreamStart(e.to_string()))?;

                    audio_client.start_stream().map_err(|e| CaptureError::StreamStart(e.to_string()))?;

                    let format = AudioFormat { sample_rate, channels: channels as u8 };
                    let _ = ready_tx.send(Ok(format));

                    let mut sample_queue: VecDeque<u8> = VecDeque::with_capacity(blockalign as usize * 8192);
                    while !stop_thread.load(Ordering::Relaxed) {
                        if event_handle.wait_for_event(200).is_err() {
                            continue; // timed out waiting for the next buffer; check stop flag and retry
                        }
                        capture_client
                            .read_from_device_to_deque(&mut sample_queue)
                            .map_err(|e| CaptureError::Device(e.to_string()))?;
                        while sample_queue.len() >= 4 {
                            let bytes = [
                                sample_queue.pop_front().unwrap(),
                                sample_queue.pop_front().unwrap(),
                                sample_queue.pop_front().unwrap(),
                                sample_queue.pop_front().unwrap(),
                            ];
                            if producer.push(f32::from_le_bytes(bytes)).is_err() {
                                break; // ring full (downstream stalled): drop the rest of this buffer
                            }
                        }
                    }
                    Ok(format)
                })();

                if let Err(e) = run {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .expect("failed to spawn WASAPI loopback capture thread");

        let format = ready_rx
            .recv()
            .map_err(|_| CaptureError::StreamStart("capture thread exited before initializing".into()))??;

        Ok((Box::new(ThreadCaptureHandle::new(stop, join)), format))
    }
}
