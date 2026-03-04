// solage_web/src/lib.rs

use wasm_bindgen::prelude::*;
use solage_core::NoAuth;
use solage_ui::SolageApp;
use std::path::PathBuf;

#[cfg(target_arch = "wasm32")]
use solage_core::PlatformBackend;



#[cfg(target_arch = "wasm32")]
struct WebBackend;

#[cfg(target_arch = "wasm32")]
impl PlatformBackend for WebBackend {
    fn pick_file(&self) -> Option<PathBuf> { None }
    fn save_file(&self, _path: &PathBuf, _content: &str) -> Result<(), String> {
        Err("Non supporté".to_string())
    }
    fn launch_external(&self, _cmd: &str, _args: &[&str]) -> Result<(), String> { Ok(()) }
    fn get_config_dir(&self) -> PathBuf { PathBuf::from("/local_storage") }
}

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();

    #[cfg(target_arch = "wasm32")]
    {
        eframe::WebLogger::init(log::LevelFilter::Debug).ok();
        log::info!("🚀 Solage Web démarrage...");

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
                    Box::new(|cc| Ok(Box::new(SolageApp::new(cc, Box::new(WebBackend), Box::new(NoAuth::new()))))),
                )
                .await;

            if let Err(e) = start_result {
                log::error!("Erreur au lancement d'eframe: {:?}", e);
            }
        });
    }
}