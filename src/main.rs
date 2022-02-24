#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
#[cfg(not(debug_assertions))]
mod dbus;

mod ui;
mod spotify;

fn main() {
    let app = ui::EspotApp::default();
    let native_options = eframe::NativeOptions::default();
    
    eframe::run_native(Box::new(app), native_options);
}
