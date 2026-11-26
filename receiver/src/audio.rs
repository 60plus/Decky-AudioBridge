//! The sound card side. One thread owns the output stream, reopens it when
//! the device changes or disappears, and keeps the device list fresh.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::log;
use crate::net::lock;
use crate::playout::Playout;

#[derive(Debug, Clone, Default)]
pub struct AudioStatus {
    pub device: Option<String>,
    pub rate: u32,
    pub channels: u16,
    pub error: Option<String>,
    pub devices: Vec<String>,
    pub default_device: Option<String>,
    pub fell_back: bool,
}

/// Called with (device name, sample rate) whenever a stream opens.
pub type OnOpen = Box<dyn Fn(&str, u32) + Send + Sync>;

pub struct Shared {
    pub playout: Arc<Mutex<Playout>>,
    pub wanted: Mutex<Option<String>>,
    pub rebuild: AtomicBool,
    pub status: Mutex<AudioStatus>,
    pub on_open: OnOpen,
    stop: AtomicBool,
}

impl Shared {
    pub fn new(playout: Arc<Mutex<Playout>>, wanted: Option<String>, on_open: OnOpen) -> Self {
        Self {
            playout,
            wanted: Mutex::new(wanted),
            rebuild: AtomicBool::new(false),
            status: Mutex::new(AudioStatus::default()),
            on_open,
            stop: AtomicBool::new(false),
        }
    }

    pub fn select(&self, device: Option<String>) {
        *lock(&self.wanted) = device;
        self.rebuild.store(true, Ordering::SeqCst);
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub fn start(shared: Arc<Shared>) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || run(shared))
        .ok()
}

struct Open {
    /// Kept only so the stream lives as long as this value.
    _stream: cpal::Stream,
    device: String,
    rate: u32,
    channels: u16,
    fell_back: bool,
}

fn run(shared: Arc<Shared>) {
    let host = cpal::default_host();
    let failed = Arc::new(AtomicBool::new(false));
    let mut current: Option<Open> = None;
    let mut last_scan: Option<Instant> = None;
    let mut retry_at: Option<Instant> = None;
    let mut known_default: Option<String> = None;

    while !shared.stop.load(Ordering::SeqCst) {
        if last_scan
            .map(|t| t.elapsed() > Duration::from_secs(3))
            .unwrap_or(true)
        {
            last_scan = Some(Instant::now());
            let devices: Vec<String> = host
                .output_devices()
                .map(|it| it.filter_map(|d| d.name().ok()).collect())
                .unwrap_or_default();
            let default = host.default_output_device().and_then(|d| d.name().ok());
            let follows_default = lock(&shared.wanted).is_none();
            if follows_default && current.is_some() && default.is_some() && default != known_default
            {
                log!(
                    "Windows default output changed to {}",
                    default.clone().unwrap_or_default()
                );
                shared.rebuild.store(true, Ordering::SeqCst);
            }
            // A chosen device that comes back after being unplugged.
            if let (Some(open), Some(wanted)) = (&current, lock(&shared.wanted).clone()) {
                if open.fell_back && devices.contains(&wanted) {
                    shared.rebuild.store(true, Ordering::SeqCst);
                }
            }
            known_default = default.clone();
            let mut st = lock(&shared.status);
            st.devices = devices;
            st.default_device = default;
        }

        let need = current.is_none()
            || shared.rebuild.load(Ordering::SeqCst)
            || failed.load(Ordering::SeqCst);
        if need && retry_at.map(|t| Instant::now() >= t).unwrap_or(true) {
            shared.rebuild.store(false, Ordering::SeqCst);
            failed.store(false, Ordering::SeqCst);
            drop(current.take());
            let wanted = lock(&shared.wanted).clone();
            match open(&host, wanted.as_deref(), &shared.playout, failed.clone()) {
                Ok(o) => {
                    log!(
                        "playing on \"{}\" at {} Hz, {} channels",
                        o.device,
                        o.rate,
                        o.channels
                    );
                    (shared.on_open)(&o.device, o.rate);
                    let mut st = lock(&shared.status);
                    st.device = Some(o.device.clone());
                    st.rate = o.rate;
                    st.channels = o.channels;
                    st.error = None;
                    st.fell_back = o.fell_back;
                    retry_at = None;
                    current = Some(o);
                }
                Err(message) => {
                    log!("cannot open output: {message}");
                    let mut st = lock(&shared.status);
                    st.error = Some(message);
                    st.device = None;
                    retry_at = Some(Instant::now() + Duration::from_secs(2));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(current);
}

fn find_device(host: &cpal::Host, wanted: Option<&str>) -> Result<(cpal::Device, bool), String> {
    if let Some(name) = wanted {
        if let Ok(mut devices) = host.output_devices() {
            if let Some(device) = devices.find(|d| d.name().ok().as_deref() == Some(name)) {
                return Ok((device, false));
            }
        }
        let device = host
            .default_output_device()
            .ok_or("No sound output device found.")?;
        return Ok((device, true));
    }
    let device = host
        .default_output_device()
        .ok_or("No sound output device found.")?;
    Ok((device, false))
}

fn open(
    host: &cpal::Host,
    wanted: Option<&str>,
    playout: &Arc<Mutex<Playout>>,
    failed: Arc<AtomicBool>,
) -> Result<Open, String> {
    let (device, fell_back) = find_device(host, wanted)?;
    let name = device.name().unwrap_or_else(|_| "Unknown device".into());
    let supported = device
        .default_output_config()
        .map_err(|e| format!("\"{name}\" cannot be used: {e}"))?;
    let rate = supported.sample_rate().0;
    let channels = supported.channels();
    let config: cpal::StreamConfig = supported.config();
    lock(playout).set_out_rate(rate);
    let ch = channels as usize;
    let on_error = move |err: cpal::StreamError| {
        log!("output stream error: {err}");
        failed.store(true, Ordering::SeqCst);
    };
    let pl = playout.clone();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| lock(&pl).pull(data, ch),
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => {
            let mut scratch: Vec<f32> = Vec::new();
            device.build_output_stream(
                &config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    scratch.resize(data.len(), 0.0);
                    lock(&pl).pull(&mut scratch, ch);
                    for (out, s) in data.iter_mut().zip(&scratch) {
                        *out = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }
                },
                on_error,
                None,
            )
        }
        cpal::SampleFormat::I32 => {
            let mut scratch: Vec<f32> = Vec::new();
            device.build_output_stream(
                &config,
                move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
                    scratch.resize(data.len(), 0.0);
                    lock(&pl).pull(&mut scratch, ch);
                    for (out, s) in data.iter_mut().zip(&scratch) {
                        *out = (s.clamp(-1.0, 1.0) as f64 * 2_147_483_647.0) as i32;
                    }
                },
                on_error,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let mut scratch: Vec<f32> = Vec::new();
            device.build_output_stream(
                &config,
                move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
                    scratch.resize(data.len(), 0.0);
                    lock(&pl).pull(&mut scratch, ch);
                    for (out, s) in data.iter_mut().zip(&scratch) {
                        *out = ((s.clamp(-1.0, 1.0) + 1.0) * 32767.5) as u16;
                    }
                },
                on_error,
                None,
            )
        }
        other => {
            return Err(format!(
                "\"{name}\" uses an unsupported sample format ({other:?})."
            ))
        }
    }
    .map_err(|e| format!("Cannot open \"{name}\": {e}"))?;
    stream
        .play()
        .map_err(|e| format!("Cannot start \"{name}\": {e}"))?;
    Ok(Open {
        _stream: stream,
        device: name,
        rate,
        channels,
        fell_back,
    })
}
