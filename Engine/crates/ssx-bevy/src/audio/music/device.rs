use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ssx_assets::sfx::output::feed::{ALSA_ORDER, DeviceFeed, DeviceFormat, IN_ORDER};
use ssx_assets::sfx::output::sim::{SimDevice, SimSpec};
use ssx_assets::sfx::output::{Output, RATE, stats};

// PORT(choice): 0.05 s of device buffer; tuning: the player logs' `audio:` lines from more machines
// (`SSX_AUDIO_BUFFER=<frames>` tries another); the console's XAudio takes 256-sample frames.
const BUFFER_SECONDS: f64 = 0.05;

#[derive(Default)]
struct Shared {
    callbacks: AtomicU64,
    first: AtomicUsize,
    largest: AtomicUsize,
    lost: AtomicBool,
    error: std::sync::Mutex<Option<String>>,
}

enum Stream {
    Device { _stream: cpal::Stream },
    Sim { _device: SimDevice },
}

#[derive(Resource)]
pub struct AudioDevice {
    output: Output,
    stream: Option<Stream>,
    shared: Arc<Shared>,
    opened: Option<(String, Instant)>,
    real: bool,
    retry_at: Option<Instant>,
    seen: (u64, Instant),
    reporter: stats::Reporter,
    capture: Option<PathBuf>,
    opens: u32,
    failure: Option<String>,
}

fn buffer_frames(rate: u32) -> u32 {
    let env = std::env::var("SSX_AUDIO_BUFFER").ok().and_then(|v| v.parse::<u32>().ok()).filter(|n| *n >= 64);
    env.unwrap_or_else(|| {
        let n = (f64::from(rate) * BUFFER_SECONDS) as u32;
        let up = n.next_power_of_two();
        if n - up / 2 <= up - n { up / 2 } else { up }
    })
}

fn candidates(device: &cpal::Device) -> Vec<(cpal::StreamConfig, cpal::SampleFormat)> {
    let Ok(default) = device.default_output_config() else { return Vec::new() };
    let mut rates = Vec::new();
    let takes_mixer_rate = device.supported_output_configs().is_ok_and(|mut it| {
        it.any(|r| {
            r.channels() == default.channels()
                && r.sample_format() == default.sample_format()
                && (r.min_sample_rate()..=r.max_sample_rate()).contains(&RATE)
        })
    });
    if takes_mixer_rate {
        rates.push(RATE);
    }
    if !rates.contains(&default.sample_rate()) {
        rates.push(default.sample_rate());
    }
    let mut out = Vec::new();
    for rate in rates {
        for buffer_size in [cpal::BufferSize::Fixed(buffer_frames(rate)), cpal::BufferSize::Default] {
            let config = cpal::StreamConfig { channels: default.channels(), sample_rate: rate, buffer_size };
            out.push((config, default.sample_format()));
        }
    }
    out
}

fn fill<T>(data: &mut cpal::Data, feed: &mut DeviceFeed, scratch: &mut Vec<f32>) -> usize
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let Some(out) = data.as_slice_mut::<T>() else { return 0 };
    scratch.resize(out.len(), 0.0);
    feed.fill(scratch);
    for (o, s) in out.iter_mut().zip(scratch.iter()) {
        *o = T::from_sample(*s);
    }
    out.len()
}

fn build(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    mut feed: DeviceFeed,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, String> {
    use cpal::SampleFormat as F;
    let channels = usize::from(config.channels.max(1));
    let (data_shared, mut scratch) = (shared.clone(), Vec::new());
    let data = move |data: &mut cpal::Data, _: &cpal::OutputCallbackInfo| {
        let frames = data.len() / channels;
        if data_shared.callbacks.fetch_add(1, Relaxed) == 0 {
            data_shared.first.store(frames, Relaxed);
        }
        if data_shared.largest.fetch_max(frames, Relaxed) < frames {
            let buffer = if cfg!(windows) {
                frames
            } else if cfg!(target_os = "linux") {
                2 * frames
            } else {
                0
            };
            feed.set_buffer(buffer, cfg!(windows));
        }
        let (feed, scratch) = (&mut feed, &mut scratch);
        let samples = match data.sample_format() {
            F::F32 => data.as_slice_mut::<f32>().map_or(0, |out| {
                feed.fill(out);
                out.len()
            }),
            F::I16 => fill::<i16>(data, feed, scratch),
            F::I32 => fill::<i32>(data, feed, scratch),
            F::I24 => fill::<cpal::I24>(data, feed, scratch),
            F::U16 => fill::<u16>(data, feed, scratch),
            F::I8 => fill::<i8>(data, feed, scratch),
            F::U8 => fill::<u8>(data, feed, scratch),
            F::U24 => fill::<cpal::U24>(data, feed, scratch),
            F::U32 => fill::<u32>(data, feed, scratch),
            F::I64 => fill::<i64>(data, feed, scratch),
            F::U64 => fill::<u64>(data, feed, scratch),
            F::F64 => fill::<f64>(data, feed, scratch),
            _ => 0,
        };
        if samples == 0 {
            scratch.resize(data.len(), 0.0);
            feed.fill(scratch);
        }
    };
    let error = move |e: cpal::StreamError| match e {
        cpal::StreamError::BufferUnderrun => {
            stats::UNDERRUNS.fetch_add(1, Relaxed);
        }
        e => {
            if matches!(e, cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated) {
                shared.lost.store(true, Relaxed);
            }
            stats::ERRORS.fetch_add(1, Relaxed);
            if let Ok(mut last) = shared.error.lock() {
                *last = Some(e.to_string());
            }
        }
    };
    let stream = device.build_output_stream_raw(config, format, data, error, None).map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

fn surround_order(host: &str, channels: u16) -> Option<[usize; ssx_assets::sfx::output::DAC_CHANNELS]> {
    if usize::from(channels) < ssx_assets::sfx::output::DAC_CHANNELS {
        return None;
    }
    match host {
        "Wasapi" | "CoreAudio" => Some(IN_ORDER),
        "Alsa" => Some(ALSA_ORDER),
        _ => None,
    }
}

fn open_device(
    output: &Output,
    shared: &Arc<Shared>,
    capture: Option<&PathBuf>,
) -> Result<(cpal::Stream, String), String> {
    let host = cpal::default_host();
    let mut devices: Vec<cpal::Device> = host.default_output_device().into_iter().collect();
    if devices.is_empty() {
        return Err("no default output device".into());
    }
    let default_id = devices[0].id().ok();
    let others = host.output_devices().into_iter().flatten().filter(|d| {
        d.description().is_ok_and(|x| x.driver().is_some_and(|driver| driver != "null")) && d.id().ok() != default_id
    });
    devices.extend(others);
    let mut first_error = None;
    for device in &devices {
        let name = device.description().map_or_else(|_| "(unnamed)".into(), |d| d.name().to_string());
        for (config, format) in candidates(device) {
            let asked = match config.buffer_size {
                cpal::BufferSize::Fixed(n) => n as usize,
                cpal::BufferSize::Default => 0,
            };
            let device_format = DeviceFormat {
                rate: config.sample_rate,
                channels: usize::from(config.channels),
                buffer: 0,
                period: asked,
                surround: surround_order(host.id().name(), config.channels),
            };
            let feed = DeviceFeed::new(output, device_format, capture.map(PathBuf::as_path));
            match build(device, &config, format, feed, shared.clone()) {
                Ok(stream) => {
                    let convert = if config.sample_rate == RATE {
                        "the mixer's rate".to_string()
                    } else {
                        format!("converted from the mixer's {RATE} Hz")
                    };
                    let buffer = if asked > 0 { format!("{asked} frames asked") } else { "the device's own".into() };
                    let channels = if surround_order(host.id().name(), config.channels).is_some() {
                        "the game's 5.1"
                    } else {
                        "the stereo fold"
                    };
                    let line = format!(
                        "\"{name}\" ({}), {} Hz ({convert}), {} channels ({channels}), {format}, buffer: {buffer}",
                        host.id().name(),
                        config.sample_rate,
                        config.channels
                    );
                    return Ok((stream, line));
                }
                Err(e) => {
                    first_error.get_or_insert(format!("\"{name}\" at {} Hz: {e}", config.sample_rate));
                }
            }
        }
    }
    Err(first_error.unwrap_or_else(|| "no output device with a default configuration".into()))
}

const SILENT_STREAM: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_secs(5);

impl AudioDevice {
    pub fn start(output: Output) -> Self {
        let capture = std::env::var_os("SSX_AUDIO_CAPTURE").filter(|v| !v.is_empty()).map(PathBuf::from);
        let mut device = Self {
            output,
            stream: None,
            shared: Arc::default(),
            opened: None,
            real: true,
            retry_at: None,
            seen: (0, Instant::now()),
            reporter: stats::Reporter::default(),
            capture,
            opens: 0,
            failure: None,
        };
        device.open();
        device
    }

    fn capture_path(&self) -> Option<PathBuf> {
        let path = self.capture.clone()?;
        if self.opens == 0 {
            return Some(path);
        }
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        Some(path.with_file_name(format!("{stem}-{}.wav", self.opens + 1)))
    }

    fn open(&mut self) {
        if matches!(self.stream, Some(Stream::Device { .. })) {
            self.stream = None;
        }
        self.shared = Arc::default();
        self.seen = (0, Instant::now());
        let capture = self.capture_path();
        let spec = std::env::var("SSX_AUDIO_DEVICE").ok().filter(|v| !v.is_empty());
        let sim = |spec: SimSpec, output: &Output, capture: Option<&PathBuf>| {
            let feed = DeviceFeed::new(output, spec.format(), capture.map(PathBuf::as_path));
            SimDevice::start(spec, feed).map(|d| Stream::Sim { _device: d }).map_err(|e| e.to_string())
        };
        let parsed = spec.as_deref().map(SimSpec::parse);
        let opened = match parsed {
            Some(Some(spec)) => {
                self.real = false;
                sim(spec, &self.output, capture.as_ref()).map(|s| (s, format!("simulated device {}", spec.label())))
            }
            Some(None) => {
                self.real = false;
                Err(format!("SSX_AUDIO_DEVICE={}: not a simulated device", spec.unwrap_or_default()))
            }
            None => open_device(&self.output, &self.shared, capture.as_ref())
                .map(|(s, line)| (Stream::Device { _stream: s }, line)),
        };
        match opened {
            Ok((stream, line)) => {
                self.stream = Some(stream);
                self.opened = Some((line, Instant::now()));
                self.failure = None;
                self.reporter = stats::Reporter::default();
            }
            Err(e) => {
                if self.failure.as_deref() != Some(e.as_str()) {
                    warn!("audio: no sound device opens ({e}): the game runs silent");
                }
                self.failure = Some(e);
                if self.stream.is_none() {
                    self.stream = sim(SimSpec::null(), &self.output, capture.as_ref()).ok();
                }
                self.retry_at = self.real.then(|| Instant::now() + RETRY);
            }
        }
        self.opens += 1;
    }
}

pub fn watch(mut device: ResMut<AudioDevice>) {
    let d = &mut *device;
    let callbacks = d.shared.callbacks.load(Relaxed);
    if let Some((line, at)) = &d.opened {
        let first = d.shared.first.load(Relaxed);
        if matches!(d.stream, Some(Stream::Sim { .. })) {
            info!("audio: {line}; lead {} frames beyond a pull", stats::LEAD.load(Relaxed));
            d.opened = None;
        } else if callbacks > 0 {
            info!(
                "audio: {line}, the first callback {first} frames; lead {} frames beyond a pull",
                stats::LEAD.load(Relaxed)
            );
            d.opened = None;
        } else if at.elapsed() > SILENT_STREAM {
            warn!("audio: {line}, but no callback in {:.0} s", at.elapsed().as_secs_f32());
            d.opened = None;
        }
    }
    if let Some(line) = d.reporter.poll() {
        warn!("{line}");
        if let Some(e) = d.shared.error.lock().ok().and_then(|mut e| e.take()) {
            warn!("audio: the stream's last error: {e}");
        }
    }
    if !matches!(d.stream, Some(Stream::Device { .. })) {
        if d.real && d.retry_at.is_some_and(|t| Instant::now() >= t) {
            d.open();
        }
        return;
    }
    if callbacks != d.seen.0 {
        d.seen = (callbacks, Instant::now());
    }
    let lost = d.shared.lost.load(Relaxed);
    if (lost || d.seen.1.elapsed() > SILENT_STREAM) && d.retry_at.is_none_or(|t| Instant::now() >= t) {
        if d.opens <= 4 {
            let why = if lost { "its device is gone" } else { "no callback for 2 s" };
            warn!("audio: the stream stopped ({why}): opening the default device again");
        }
        d.open();
        d.retry_at = Some(Instant::now() + RETRY);
    }
}
