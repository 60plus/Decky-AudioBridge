//! The receiver window.

use std::net::Ipv4Addr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};

use crate::audio;
use crate::log;
use crate::net::{self, lock};
use crate::netinfo::{self, Adapter};
use crate::playout::{Phase, Playout};
use crate::settings::{self, Delay, Settings};
use crate::system;
use crate::vban::DEFAULT_PORT;

const GREEN: Color32 = Color32::from_rgb(94, 194, 106);
const AMBER: Color32 = Color32::from_rgb(240, 180, 41);
const RED: Color32 = Color32::from_rgb(239, 91, 91);
const BLUE: Color32 = Color32::from_rgb(90, 167, 255);
const GREY: Color32 = Color32::from_rgb(139, 146, 154);

pub struct App {
    settings: Settings,
    playout: Arc<Mutex<Playout>>,
    net: Arc<net::Shared>,
    listener: Option<net::Listener>,
    audio: Arc<audio::Shared>,
    autostart: bool,
    adapters: Vec<Adapter>,
    main_ip: Option<Ipv4Addr>,
    last_scan: Instant,
    meter: f32,
    port_text: String,
    notice: Option<(String, Instant)>,
    minimize_once: bool,
}

struct View {
    colour: Color32,
    title: String,
    detail: String,
}

fn gain_for(volume: f32) -> f32 {
    volume * volume
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, start_minimized: bool) -> Self {
        style(&cc.egui_ctx);
        let settings = settings::load();
        let mut playout = Playout::new(
            settings.delay.target_ms(None),
            settings.delay == Delay::Auto,
        );
        playout.gain = gain_for(settings.volume);
        playout.muted = settings.muted;
        let playout = Arc::new(Mutex::new(playout));
        let net = Arc::new(net::Shared::new(
            playout.clone(),
            settings.delay,
            settings.answer_discovery,
        ));

        let net_for_audio = net.clone();
        let audio = Arc::new(audio::Shared::new(
            playout.clone(),
            settings.output_device.clone(),
            Box::new(move |device, rate| {
                *lock(&net_for_audio.device_name) = device.to_string();
                *lock(&net_for_audio.output_rate) = rate;
            }),
        ));
        audio::start(audio.clone());
        let listener = Some(net::start(
            net.clone(),
            settings.listen.clone(),
            settings.port,
        ));
        let port_text = settings.port.to_string();
        let minimize_once = start_minimized || settings.start_minimized;
        Self {
            settings,
            playout,
            net,
            listener,
            audio,
            autostart: system::autostart_enabled(),
            adapters: netinfo::adapters(),
            main_ip: netinfo::main_ip(),
            last_scan: Instant::now(),
            meter: 0.0,
            port_text,
            notice: None,
            minimize_once,
        }
    }

    fn save(&mut self) {
        if let Err(e) = settings::save(&self.settings) {
            log!("cannot save settings: {e}");
            self.notify(format!("Could not save settings: {e}"));
        }
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    fn listening(&self) -> bool {
        self.listener.is_some()
    }

    fn start_listening(&mut self) {
        if self.listener.is_none() {
            *lock(&self.net.status) = net::NetStatus::default();
            self.listener = Some(net::start(
                self.net.clone(),
                self.settings.listen.clone(),
                self.settings.port,
            ));
        }
    }

    fn stop_listening(&mut self) {
        if let Some(l) = self.listener.take() {
            l.stop();
        }
        lock(&self.playout).sender_gone();
    }

    fn restart_listening(&mut self) {
        let was = self.listening();
        self.stop_listening();
        if was {
            self.start_listening();
        }
    }

    fn shown_ip(&self) -> Option<Ipv4Addr> {
        let listening_on = lock(&self.net.status).listening_on;
        match listening_on {
            Some(addr) if !addr.ip().is_unspecified() => Some(*addr.ip()),
            _ => self.main_ip,
        }
    }

    fn view(&self) -> View {
        let st = lock(&self.net.status).clone();
        let audio = lock(&self.audio.status).clone();
        let (phase, buffered, target, recent) = {
            let mut p = lock(&self.playout);
            let recent = p.recent_underruns();
            (p.phase(), p.buffered_ms(), p.target_ms(), recent)
        };
        if !self.listening() {
            return View {
                colour: GREY,
                title: "Receiver is off".into(),
                detail: "Press Start receiving to listen for your gaming PC.".into(),
            };
        }
        if let Some(err) = &st.bind_error {
            return View {
                colour: RED,
                title: "Cannot listen".into(),
                detail: err.clone(),
            };
        }
        if let Some(err) = &audio.error {
            return View {
                colour: RED,
                title: "Cannot play sound".into(),
                detail: format!("{err} Choose another output device below."),
            };
        }
        if st.playing() {
            let sender = st.sender.as_ref().unwrap();
            let who = match &sender.name {
                Some(name) => format!("{name} ({})", sender.ip),
                None => sender.ip.to_string(),
            };
            let format = st
                .format
                .map(|(rate, ch, f)| {
                    let layout = match ch {
                        1 => "mono".to_string(),
                        2 => "stereo".to_string(),
                        n => format!("{n} channels"),
                    };
                    format!("{:.1} kHz, {layout}, {}", rate as f64 / 1000.0, f.label())
                })
                .unwrap_or_default();
            if phase == Phase::Buffering || recent > 0 {
                return View {
                    colour: AMBER,
                    title: format!("Playing from {who}"),
                    detail: format!("The connection is unsteady, so the delay is now {target:.0} ms. A cable helps. {format}"),
                };
            }
            return View {
                colour: GREEN,
                title: format!("Playing from {who}"),
                detail: format!("{format}. Delay buffer {buffered:.0} ms."),
            };
        }
        if let Some((at, name, ip)) = &st.last_ping {
            if at.elapsed() < Duration::from_secs(3) {
                return View {
                    colour: BLUE,
                    title: format!("Connected to {name}"),
                    detail: format!("{ip} is sending no sound yet. Start streaming in the plugin, or play something."),
                };
            }
        }
        let address = self
            .shown_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "this PC's address".into());
        View {
            colour: BLUE,
            title: "Waiting for your gaming PC".into(),
            detail: format!("In the Audio Bridge plugin, choose Find receivers on my network, or enter {address}."),
        }
    }

    fn diagnostics(&self) -> String {
        let st = lock(&self.net.status).clone();
        let audio = lock(&self.audio.status).clone();
        let mut p = lock(&self.playout);
        let recent = p.recent_underruns();
        format!(
            "Audio Bridge Receiver {}\nListening on: {}\nSender: {}\nFormat: {}\nOutput: {} ({} Hz, {} channels)\n\
             Buffer: {:.0} ms of {:.0} ms target ({:?})\nPackets: {}, lost {}, late {}, rejected {}, other senders {}\n\
             Underruns: {} ({} in the last 10 s), skips: {}\nLog file: {}",
            env!("CARGO_PKG_VERSION"),
            st.listening_on.map(|a| a.to_string()).unwrap_or_else(|| "not listening".into()),
            st.sender.as_ref().or(st.last_sender.as_ref()).map(|s| format!("{}:{} \"{}\"", s.ip, s.port, s.stream)).unwrap_or_else(|| "none yet".into()),
            st.format.map(|(r, c, f)| format!("{r} Hz, {c} ch, {}", f.label())).unwrap_or_else(|| "-".into()),
            audio.device.unwrap_or_else(|| "none".into()),
            audio.rate,
            audio.channels,
            p.buffered_ms(),
            p.target_ms(),
            p.phase(),
            st.packets,
            p.stats.lost_packets,
            p.stats.late_packets,
            st.rejected,
            st.ignored_other,
            p.stats.underruns,
            recent,
            p.stats.skips,
            crate::logfile::path().display()
        )
    }

    fn status_box(&mut self, ui: &mut egui::Ui) {
        let view = self.view();
        let peak = lock(&self.playout).take_peak();
        self.meter = peak.max(self.meter * 0.85);
        egui::Frame::none()
            .fill(Color32::from_rgb(32, 36, 42))
            .stroke(egui::Stroke::new(2.0_f32, view.colour))
            .rounding(8.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(14.0, 22.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 6.0, view.colour);
                    ui.label(
                        RichText::new(&view.title)
                            .size(19.0)
                            .strong()
                            .color(view.colour),
                    );
                });
                ui.add_space(4.0);
                ui.label(RichText::new(view.detail).size(14.0));
                ui.add_space(8.0);
                let db = if self.meter > 0.0 {
                    20.0 * self.meter.log10()
                } else {
                    -90.0
                };
                let fraction = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_height(8.0)
                        .fill(view.colour),
                );
            });
    }

    fn address_line(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("This PC:").size(15.0));
            let ip = self.shown_ip();
            let text = match ip {
                Some(ip) if self.settings.port == DEFAULT_PORT => ip.to_string(),
                Some(ip) => format!("{ip}:{}", self.settings.port),
                None => "no network".into(),
            };
            ui.label(RichText::new(&text).size(18.0).strong().monospace());
            if ip.is_some() && ui.button("Copy").clicked() {
                ctx.copy_text(text.clone());
                self.notify("Address copied.");
            }
        });
    }

    fn output_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Output");
        let audio = lock(&self.audio.status).clone();
        let default_label = format!(
            "System default ({})",
            audio
                .default_device
                .clone()
                .unwrap_or_else(|| "none".into())
        );
        let selected_label = self
            .settings
            .output_device
            .clone()
            .unwrap_or_else(|| default_label.clone());
        let mut choice = self.settings.output_device.clone();
        egui::ComboBox::from_id_salt("device")
            .width(ui.available_width() - 8.0)
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut choice, None, default_label.clone());
                for name in &audio.devices {
                    ui.selectable_value(&mut choice, Some(name.clone()), name);
                }
            });
        if choice != self.settings.output_device {
            self.settings.output_device = choice.clone();
            self.audio.select(choice);
            self.save();
        }
        if audio.fell_back {
            ui.label(
                RichText::new(
                    "The chosen device is not connected, playing on the default device for now.",
                )
                .color(AMBER),
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Volume");
            ui.spacing_mut().slider_width = (ui.available_width() - 170.0).max(80.0);
            let mut percent = (self.settings.volume * 100.0).round();
            let slider = ui.add(
                egui::Slider::new(&mut percent, 0.0..=100.0)
                    .suffix("%")
                    .fixed_decimals(0),
            );
            if slider.changed() {
                self.settings.volume = percent / 100.0;
                lock(&self.playout).gain = gain_for(self.settings.volume);
            }
            if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
                self.save();
            }
            let label = if self.settings.muted {
                "Unmute"
            } else {
                "Mute"
            };
            if ui
                .add(egui::Button::new(label).min_size(egui::vec2(72.0, 0.0)))
                .clicked()
            {
                self.settings.muted = !self.settings.muted;
                lock(&self.playout).muted = self.settings.muted;
                self.save();
            }
        });
        if self.settings.muted {
            ui.label(RichText::new("Sound is muted on this PC.").color(AMBER));
        } else if self.settings.volume == 0.0 {
            ui.label(RichText::new("Volume is at 0%.").color(AMBER));
        }
    }

    fn delay_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Delay");
        let before = self.settings.delay;
        ui.horizontal_wrapped(|ui| {
            for delay in Delay::ALL {
                ui.radio_value(&mut self.settings.delay, delay, delay.label());
            }
        });
        ui.label(RichText::new(self.settings.delay.help()).weak());
        if self.settings.delay != before {
            *lock(&self.net.delay) = self.settings.delay;
            self.net.apply_delay();
            self.save();
        }
    }

    fn settings_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Settings").default_open(false).show(ui, |ui| {
            if system::AUTOSTART_SUPPORTED {
                let mut autostart = self.autostart;
                if ui.checkbox(&mut autostart, "Start with Windows").changed() {
                    match system::set_autostart(autostart) {
                        Ok(()) => self.autostart = autostart,
                        Err(e) => self.notify(format!("Could not change start with Windows: {e}")),
                    }
                }
            }
            if ui.checkbox(&mut self.settings.start_minimized, "Start minimized").changed() {
                self.save();
            }
            let mut answer = self.settings.answer_discovery;
            if ui
                .checkbox(&mut answer, "Let the plugin find this PC")
                .on_hover_text("Answers the plugin's search with this PC's name and output device.")
                .changed()
            {
                self.settings.answer_discovery = answer;
                self.net.answer_discovery.store(answer, Ordering::Relaxed);
                self.save();
            }
            ui.add_space(6.0);
            ui.label("Listen on");
            let label_for = |listen: &str, main: Option<Ipv4Addr>, adapters: &[Adapter]| -> String {
                match listen {
                    "auto" => format!("Main network ({})", main.map(|i| i.to_string()).unwrap_or_else(|| "none".into())),
                    "all" => "All network adapters".into(),
                    ip => adapters.iter().find(|a| a.ip.to_string() == ip).map(|a| format!("{} ({})", a.name, a.ip)).unwrap_or_else(|| ip.to_string()),
                }
            };
            let mut listen = self.settings.listen.clone();
            egui::ComboBox::from_id_salt("listen")
                .width(ui.available_width() - 8.0)
                .selected_text(label_for(&listen, self.main_ip, &self.adapters))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut listen, "auto".to_string(), label_for("auto", self.main_ip, &self.adapters));
                    for adapter in &self.adapters {
                        let ip = adapter.ip.to_string();
                        ui.selectable_value(&mut listen, ip.clone(), label_for(&ip, self.main_ip, &self.adapters));
                    }
                    ui.selectable_value(&mut listen, "all".to_string(), "All network adapters");
                });
            if listen != self.settings.listen {
                self.settings.listen = listen;
                self.save();
                self.restart_listening();
            }
            ui.horizontal(|ui| {
                ui.label("Port");
                ui.add(egui::TextEdit::singleline(&mut self.port_text).desired_width(70.0));
                if ui.button("Apply").clicked() {
                    match self.port_text.trim().parse::<u16>() {
                        Ok(port) if port >= 1024 => {
                            self.settings.port = port;
                            self.save();
                            self.restart_listening();
                            self.notify(format!("Listening on port {port}. Use the same port in the plugin."));
                        }
                        _ => self.notify("Port must be a number from 1024 to 65535."),
                    }
                }
                if ui.button("Default").clicked() {
                    self.port_text = DEFAULT_PORT.to_string();
                }
            });
            ui.add_space(6.0);
            if cfg!(windows) {
                ui.label(RichText::new(
                    "Use these if the plugin cannot find this PC, or if you pressed Cancel on the \
                     Windows Firewall question. They replace this program's own firewall rules \
                     (including a block rule left by Cancel) with one rule: incoming audio on this \
                     port, from computers on your local network only. Windows asks for permission.",
                ).weak());
                if ui
                    .button("Allow on private networks…")
                    .on_hover_text("Home and work networks that Windows marks as private.")
                    .clicked()
                {
                    if system::allow_through_firewall(self.settings.port, false) {
                        self.notify("Firewall rule added for private networks.");
                    } else {
                        self.notify("The firewall rule was not added.");
                    }
                }
                if ui
                    .button("Allow on public networks too…")
                    .on_hover_text(
                        "Only if Windows marks your home network as public. On a cafe or hotel \
                         network, others on the same network could then send sound to this PC \
                         or see its name. Setting your home network to private in Windows is better.",
                    )
                    .clicked()
                {
                    if system::allow_through_firewall(self.settings.port, true) {
                        self.notify("Firewall rule added for private and public networks.");
                    } else {
                        self.notify("The firewall rule was not added.");
                    }
                }
                if ui.button("Open log folder").clicked() {
                    system::open_folder(&settings::data_dir());
                }
            }
        });
    }

    fn diagnostics_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CollapsingHeader::new("Diagnostics")
            .default_open(false)
            .show(ui, |ui| {
                let text = self.diagnostics();
                ui.label(RichText::new(&text).monospace().size(12.0));
                if ui.button("Copy diagnostics").clicked() {
                    ctx.copy_text(text);
                    self.notify("Diagnostics copied.");
                }
            });
    }
}

fn style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.style_mut(|style| {
        use egui::{FontId, TextStyle};
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(15.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(15.0));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::proportional(18.0));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(12.0));
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.minimize_once {
            self.minimize_once = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        if self.last_scan.elapsed() > Duration::from_secs(5) {
            self.last_scan = Instant::now();
            self.adapters = netinfo::adapters();
            self.main_ip = netinfo::main_ip();
        }
        egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
            ui.add_space(6.0);
            let (label, colour) = if self.listening() {
                ("Stop receiving", Color32::from_rgb(70, 52, 52))
            } else {
                ("Start receiving", Color32::from_rgb(40, 80, 50))
            };
            let button = egui::Button::new(RichText::new(label).size(16.0))
                .fill(colour)
                .min_size(egui::vec2(ui.available_width(), 36.0));
            if ui.add(button).clicked() {
                if self.listening() {
                    self.stop_listening();
                } else {
                    self.start_listening();
                }
            }
            if let Some((text, at)) = &self.notice {
                if at.elapsed() < Duration::from_secs(6) {
                    ui.label(RichText::new(text).color(BLUE));
                }
            }
            ui.add_space(4.0);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(RichText::new("Audio Bridge Receiver").size(22.0).strong());
                    ui.add_space(4.0);
                    self.status_box(ui);
                    ui.add_space(4.0);
                    self.address_line(ui, ctx);
                    ui.separator();
                    self.output_section(ui);
                    ui.separator();
                    self.delay_section(ui);
                    ui.separator();
                    self.settings_section(ui);
                    self.diagnostics_section(ui, ctx);
                });
        });
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_listening();
        self.audio.shutdown();
        log!("receiver closed");
    }
}
