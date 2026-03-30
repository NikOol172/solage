// solage_core/src/platform.rs

use std::path::PathBuf;

pub trait PlatformBackend {
    fn save_file(&self, path: &PathBuf, content: &str) -> Result<(), String>;
    fn launch_external(&self, command: &str, args: &[&str]) -> Result<(), String>;
    fn get_config_dir(&self) -> PathBuf;
    fn default_url(&self) -> Option<String> { None }
    fn pick_file_async_mobile(&self, _tx: std::sync::mpsc::Sender<(String, String)>) {
        // Implémentation par défaut vide pour ne pas casser Desktop/Web
    }
}
