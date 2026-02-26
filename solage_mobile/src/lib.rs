// Copyright [2026] [Nicolas Houle]
// 
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// 
//     http://www.apache.org/licenses/LICENSE-2.0
// 
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use solage_core::PlatformBackend;
use solage_ui::SolageApp;
use std::path::PathBuf;

// Importation spécifique Android
#[cfg(target_os = "android")]
use android_activity::AndroidApp;

#[cfg(target_os = "android")]
use eframe::egui;

// --- CONFIGURATION DE DÉMO MOBILE ---
// On la définit ici pour que le mobile ait sa propre "personnalité"
const MOBILE_DEMO_YAML: &str = r#"
title: "Solage Mobile"
version: "1.0"
sections:
  - name: "Contrôle"
    icon: "📱"
    modes:
      - name: "Tactile"
        flavors:
          - name: "Simple"
            rows:
              - key: "status"
                label: "État"
                widget: { type: "text", default: "Prêt" }
              - key: "brightness"
                label: "Luminosité"
                widget: { type: "slider", min: 0.0, max: 100.0, default: 80.0 }
              - key: "vibrate"
                label: "Vibration"
                widget: { type: "bool", default: true }
"#;

// --- 1. IMPLÉMENTATION DU BACKEND MOBILE ---
struct MobileBackend {
    data_dir: PathBuf,
}

impl PlatformBackend for MobileBackend {
    fn pick_file(&self) -> Option<PathBuf> {
        // Sur mobile, pour l'instant, on ne supporte pas le File Picker système
        // On pourrait utiliser JNI plus tard, mais pour l'instant on retourne None.
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

// --- 2. POINT D'ENTRÉE ANDROID ---
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    use log::LevelFilter;

    // Initialisation des logs (visible via 'adb logcat')
    android_logger::init_once(
        android_logger::Config::default().with_max_level(LevelFilter::Info)
    );

    // Récupération du dossier interne de l'app (/data/data/com.nhoule.solage/...)
    let data_dir = app.internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp"));

    let options = eframe::NativeOptions {
        android_app: Some(app),
        ..Default::default()
    };

    eframe::run_native(
        "Solage Mobile",
        options,
        Box::new(move |cc| {
            // A. On crée notre backend spécifique
            let backend = MobileBackend { data_dir };
            
            // B. On initialise l'UI commune avec ce backend
            let mut solage_app = SolageApp::new(cc, Box::new(backend));
            
            // C. CRUCIAL : On injecte la config de démo immédiatement !
            // Comme on n'a pas de bouton "Ouvrir", il faut que l'app se lance avec du contenu.
            // solage_app.load_yaml_string(MOBILE_DEMO_YAML);
            
            Ok(Box::new(solage_app))
        }),
    ).unwrap();
}
