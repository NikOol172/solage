#![cfg(target_arch = "wasm32")]

use eframe::wasm_bindgen::{self, prelude::*};
use solage_core::PlatformBackend;
use solage_ui::SolageApp;
use std::path::PathBuf;

// 1. LE BACKEND WEB
struct WebBackend;

impl PlatformBackend for WebBackend {
    fn pick_file(&self) -> Option<PathBuf> {
        // Le navigateur bloque l'accès direct aux fichiers.
        // On demandera à l'utilisateur d'utiliser l'URL (ehttp).
        None
    }

    fn save_file(&self, _path: &PathBuf, _content: &str) -> Result<(), String> {
        Err("Sauvegarde locale non supportée sur le Web.".to_string())
    }

    fn launch_external(&self, _cmd: &str, _args: &[&str]) -> Result<(), String> {
        Ok(()) // Silencieux sur le web
    }

    fn get_config_dir(&self) -> PathBuf {
        PathBuf::from("/local_storage") // Fictif, eframe gère son propre stockage web
    }
}

// 2. LE LANCEMENT DANS LE NAVIGATEATEUR
fn main() {
    // Redirige les logs Rust vers la console du navigateur (F12)
    eframe::WebLogger::init(log::LevelFilter::Debug).ok();
    console_error_panic_hook::set_once(); // Affiche les crashs Rust dans la console JS

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window().unwrap().document().unwrap();
        let canvas = document
            .get_element_by_id("the_canvas_id")
            .unwrap()
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .unwrap();

        let start_result = eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| Ok(Box::new(SolageApp::new(cc, Box::new(WebBackend))))),
            )
            .await;

        if let Err(e) = start_result {
            log::error!("Erreur au lancement d'eframe: {:?}", e);
        }
    });
}
