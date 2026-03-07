use eframe::egui;
use solage_core::PlatformBackend;
use solage_core::NoAuth;
use solage_ui::SolageApp;
use std::process::Command;
use std::path::PathBuf;

struct DesktopBackend;

impl PlatformBackend for DesktopBackend {
    fn pick_file(&self) -> Option<PathBuf> {
        rfd::FileDialog::new().pick_file()
    }

    fn pick_file_async(&self, tx: std::sync::mpsc::Sender<PathBuf>) {
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new().pick_file() {
                let _ = tx.send(path);
            }
        });
    }

    fn save_file(&self, path: &PathBuf, content: &str) -> Result<(), String> {
        std::fs::write(path, content).map_err(|e| e.to_string())
    }

    fn launch_external(&self, command: &str, args: &[&str]) -> Result<(), String> {
        Command::new(command)
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|e: std::io::Error| e.to_string())
    }

    fn get_config_dir(&self) -> PathBuf {
        dirs::config_dir().unwrap_or_default().join("solage")
    }
}

fn main() -> eframe::Result<()> {
    env_logger::init();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Solage Desktop",
        native_options,
        Box::new(|cc| { 
            Ok(Box::new(SolageApp::new(cc, Box::new(DesktopBackend), Box::new(NoAuth::new()))))
        }),
    )
}
