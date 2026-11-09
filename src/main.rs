//! QO-ALL - full-duplex SSB transceiver for QO-100.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod ftpanel;
mod logpanel;
mod settings;
mod ui;

use eframe::egui;

/// Without a bundled libiio (development), the copy of an installed PothosSDR is used.
/// Its dependencies (libusb-1.0.dll and others) are looked up along PATH, where other
/// radio programs often put older copies first; PothosSDR's folder is moved to the front.
fn prefer_pothos_dlls() {
    if !cfg!(windows) {
        return;
    }
    let bundled = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("libiio.dll").is_file())).unwrap_or(false);
    let base = std::env::var_os("ProgramFiles").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Program Files".into());
    let pothos = base.join("PothosSDR").join("bin");
    if bundled || !pothos.is_dir() {
        return;
    }
    let mut paths = vec![pothos];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        // Set while there are no other threads yet.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

/// `qo-all --diagnose`: what the program finds, on standard output.
fn diagnose() {
    println!("QO-ALL {}", env!("CARGO_PKG_VERSION"));
    println!("libiio: {:?}", qo_all::iio::library_status());
    for (uri, description) in qo_all::iio::scan_usb() {
        println!("  USB: {uri} | {description}");
    }
    for d in qo_all::audio::input_devices() {
        println!("  microphone: {}", d.name);
    }
    for d in qo_all::audio::output_devices() {
        println!("  speaker: {}", d.name);
    }
    if let Some(uri) = std::env::args().skip_while(|a| a != "--diagnose").nth(1) {
        match qo_all::iio::IioRadio::open(&uri) {
            Ok(radio) => {
                println!("{uri}: {:?}", radio.identity());
                if let Some(w) = &radio.fir_warning {
                    println!("warning: {w}");
                }
                print!("{}", radio.describe_tx());
                // Opening the transmitter leaves it powered down and fully attenuated.
                println!("transmitter: {:?}", radio.transmitter().map(|t| t.buffer_len()));
            }
            Err(e) => println!("{uri}: {e}"),
        }
    }
}

fn main() -> eframe::Result {
    prefer_pothos_dlls();
    if std::env::args().any(|a| a == "--diagnose") {
        diagnose();
        return Ok(());
    }
    const ICON: usize = 64;
    let icon = egui::IconData { rgba: qo_all::logo::rgba(ICON, true), width: ICON as u32, height: ICON as u32 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("QO-ALL")
            .with_min_inner_size([1100.0, 700.0])
            .with_maximized(true)
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native("QO-ALL", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
