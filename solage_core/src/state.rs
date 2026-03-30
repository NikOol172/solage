// solage_core/src/state.rs

use crate::{AppState, GlobalPreferences};

#[cfg(not(target_arch = "wasm32"))]
pub fn save_state(path: &str, state: &AppState) -> Result<(), std::io::Error> {
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(path, json)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_state(path: &str) -> Result<AppState, std::io::Error> {
    let content = std::fs::read_to_string(path)?;
    let state: AppState = serde_json::from_str(&content)?;
    Ok(state)
}

#[cfg(target_arch = "wasm32")]
pub fn save_state(path: &str, state: &AppState) -> Result<(), std::io::Error> {
    if let Ok(json) = serde_json::to_string(state) {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                let key = format!("solage_state_{}", path);
                let _ = storage.set_item(&key, &json);
                return Ok(());
            }
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::Other, "Erreur écriture état Web"))
}

#[cfg(target_arch = "wasm32")]
pub fn load_state(path: &str) -> Result<AppState, std::io::Error> {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            let key = format!("solage_state_{}", path);
            if let Ok(Some(data)) = storage.get_item(&key) {
                if let Ok(state) = serde_json::from_str(&data) {
                    return Ok(state);
                }
            }
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Aucun état web"))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_preferences(path: &str) -> Result<GlobalPreferences, std::io::Error> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let prefs = serde_json::from_reader(reader)?;
    Ok(prefs)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn save_preferences(path: &str, prefs: &GlobalPreferences) -> Result<(), std::io::Error> {
    let file = std::fs::File::create(path)?;
    serde_json::to_writer_pretty(file, prefs)?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub fn load_preferences(_path: &str) -> Result<GlobalPreferences, std::io::Error> {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            if let Ok(Some(data)) = storage.get_item("solage_prefs") {
                if let Ok(prefs) = serde_json::from_str(&data) {
                    return Ok(prefs);
                }
            }
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Aucune préférence web"))
}

#[cfg(target_arch = "wasm32")]
pub fn save_preferences(_path: &str, prefs: &GlobalPreferences) -> Result<(), std::io::Error> {
    if let Ok(json) = serde_json::to_string(prefs) {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                // On sauvegarde le JSON généré directement dans la mémoire du navigateur
                let _ = storage.set_item("solage_prefs", &json);
                return Ok(());
            }
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::Other, "Erreur d'écriture Web"))
}
