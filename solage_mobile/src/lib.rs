use solage_core::{PlatformBackend, NoAuth};
use solage_ui::SolageApp;
use std::path::PathBuf;

// Importation spécifique Android
#[cfg(target_os = "android")]
use android_activity::AndroidApp;

#[cfg(target_os = "android")]
use eframe::egui;

struct MobileBackend {
    data_dir: PathBuf,
}

impl PlatformBackend for MobileBackend {
    fn pick_file(&self) -> Option<PathBuf> {
        log::info!("Pick file demandé, mais non supporté sur cette version mobile.");
        None
    }

    fn save_file(&self, _path: &PathBuf, _content: &str) -> Result<(), String> {
        log::warn!("Sauvegarde non implémentée sur mobile (Sandbox)");
        Err("Non supporté".to_string())
    }

    fn launch_external(&self, cmd: &str, _args: &[&str]) -> Result<(), String> {
        log::info!("Commande externe ignorée sur mobile : {}", cmd);
        Ok(())
    }

    fn get_config_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    use log::LevelFilter;

    android_logger::init_once(
        android_logger::Config::default().with_max_level(LevelFilter::Info)
    );

    let data_dir = app.internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp"));

    let options = eframe::NativeOptions {
        android_app: Some(app.clone()),
        ..Default::default()
    };

    app.set_window_flags(
        android_activity::WindowManagerFlags::KEEP_SCREEN_ON,
        android_activity::WindowManagerFlags::empty(),
    );

    eframe::run_native(
        "Solage Mobile",
        options,
        Box::new(move |cc| {
            let backend = MobileBackend { data_dir };      
            let mut solage_app = SolageApp::new(cc, Box::new(backend), Box::new(NoAuth::new()));;           
            Ok(Box::new(solage_app))
        }),
    ).unwrap();
}
