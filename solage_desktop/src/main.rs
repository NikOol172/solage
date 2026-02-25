use eframe::egui;
use solage_core::PlatformBackend;
use solage_ui::SolageApp; // On a seulement besoin de ça !
use std::process::Command;
use std::path::PathBuf;

// 1. On crée la structure pour le Desktop
struct DesktopBackend;

// 2. On implémente le contrat avec les vrais outils système (Windows/Linux/Mac)
impl PlatformBackend for DesktopBackend {
    fn pick_file(&self) -> Option<PathBuf> {
        rfd::FileDialog::new().pick_file() // Utilise la boite de dialogue native
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
    // Configuration pour Linux (Wayland/X11)
    unsafe {
        std::env::set_var("WINIT_UNIX_BACKEND", "x11");
        std::env::remove_var("WAYLAND_DISPLAY"); 
    }
    env_logger::init();

    let native_options = eframe::NativeOptions {
        // Optionnel : Définir la taille de départ
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Solage Desktop",
        native_options,
        Box::new(|cc| {
            // 1. On crée le backend Desktop
            let backend = DesktopBackend; 
            
            // 2. On l'injecte dans l'application unifiée
            Ok(Box::new(SolageApp::new(cc, Box::new(backend))))
        }),
    )
}
