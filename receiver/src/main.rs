#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod logfile;
mod net;
mod netinfo;
mod playout;
mod settings;
mod system;
mod ui;
mod vban;

use eframe::egui;

/// The window icon: five rounded bars, drawn here so no image decoder is needed.
fn icon() -> egui::IconData {
    const SIZE: usize = 64;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    let bars = [
        (10.0, 0.35),
        (20.0, 0.6),
        (32.0, 0.85),
        (44.0, 0.6),
        (54.0, 0.35),
    ];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let i = (y * SIZE + x) * 4;
            // dark rounded square background
            let r = 12.0f32;
            let dx = (fx - 32.0).abs() - (32.0 - r);
            let dy = (fy - 32.0).abs() - (32.0 - r);
            let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() - r;
            if outside > 0.0 {
                continue;
            }
            let (mut cr, mut cg, mut cb) = (27u8, 40u8, 56u8);
            for (cx, h) in bars {
                let half = h * 26.0;
                let ddx = (fx - cx).abs() - 0.0;
                let ddy = ((fy - 32.0).abs() - half).max(0.0);
                if (ddx.powi(2) + ddy.powi(2)).sqrt() <= 3.6 {
                    (cr, cg, cb) = (94, 194, 255);
                }
            }
            rgba[i] = cr;
            rgba[i + 1] = cg;
            rgba[i + 2] = cb;
            rgba[i + 3] = 255;
        }
    }
    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}

fn main() {
    logfile::init();
    log!(
        "Audio Bridge Receiver {} starting",
        env!("CARGO_PKG_VERSION")
    );
    std::panic::set_hook(Box::new(|info| {
        log!("crash: {info}");
    }));
    let minimized = std::env::args().any(|a| a == "--minimized");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Audio Bridge Receiver")
            .with_inner_size([470.0, 680.0])
            .with_min_inner_size([380.0, 440.0])
            .with_icon(std::sync::Arc::new(icon())),
        ..Default::default()
    };
    if let Err(err) = eframe::run_native(
        "Audio Bridge Receiver",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, minimized)))),
    ) {
        log!("cannot open the window: {err}");
        system::error_box(&format!(
            "Audio Bridge Receiver could not open its window.\n\n{err}\n\nThe graphics driver may be missing OpenGL support."
        ));
    }
}
