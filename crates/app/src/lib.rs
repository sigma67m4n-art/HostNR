#![cfg(target_os = "android")]

mod net;
mod platform;
mod ui;
mod vpn;

use winit::platform::android::activity::AndroidApp;

#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));
    if let Err(error) = platform::init(&app) {
        log::error!("Android bridge: {error}");
        return;
    }
    let root = match app.internal_data_path() { Some(p) => p.join("hostnr"), None => return };
    let options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
    if let Err(error) = eframe::run_native("HostNR", options, Box::new(move |cc| Ok(Box::new(ui::HostApp::new(cc, root))))) {
        log::error!("UI: {error}");
    }
}
