#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
#[cfg(not(debug_assertions))]
mod dbus;

mod ui;
mod spotify;

fn main() {
    let native_options = eframe::NativeOptions::default();
    
    eframe::run_native(
        "espot-rs",
        native_options,
        Box::new(| cc | Box::new(ui::EspotApp::new(cc)))
    );
}
