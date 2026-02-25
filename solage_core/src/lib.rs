pub use solage_data::*;
use rhai::{Engine, Scope}; 
use std::collections::HashMap;
use std::process::Command;
use std::path::PathBuf;
use std::fs;

pub trait PlatformBackend {
    fn pick_file(&self) -> Option<PathBuf>;
    fn save_file(&self, path: &PathBuf, content: &str) -> Result<(), String>;
    fn launch_external(&self, command: &str, args: &[&str]) -> Result<(), String>;
    fn get_config_dir(&self) -> PathBuf;
}


pub fn load_config(yaml_content: &str) -> Result<AppConfig, serde_yaml::Error> {
    serde_yaml::from_str(yaml_content)
}

// --- GESTION DE L'ÉTAT DU PROJET (VARIABLES, SLIDERS, ETC) ---

// 1. Version NATIVE : Utilise un fichier .json à côté du .yaml
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

// 2. Version WEB : Utilise le LocalStorage avec l'URL comme clé
#[cfg(target_arch = "wasm32")]
pub fn save_state(path: &str, state: &AppState) -> Result<(), std::io::Error> {
    if let Ok(json) = serde_json::to_string(state) {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                // On préfixe la clé pour ne pas la mélanger avec les préférences
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
// --- GESTION DES PRÉFÉRENCES (MULTI-PLATEFORME) ---

// 1. Version NATIVE (Desktop / Mobile) : Utilise le disque dur
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

// 2. Version WEB : Utilise le LocalStorage du navigateur (le paramètre 'path' est ignoré)
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
    // Si rien n'est trouvé, on renvoie une erreur classique pour que l'UI crée un fichier vide
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
// --- NOUVEAU : Wrapper opaque pour le Scope Rhai ---
// Cela permet à solage_ui de stocker un Scope sans importer Rhai directement
pub struct ScriptContext {
    pub(crate) scope: Scope<'static>,
}

impl ScriptContext {
    pub fn new() -> Self {
        Self { scope: Scope::new() }
    }
}

pub struct ScriptEngine {
    engine: Engine,
}

impl ScriptEngine {
    pub fn new() -> Self {
        let mut engine = Engine::new();
        engine.on_print(|msg| { println!("[SCRIPT] {}", msg); });
        engine.on_debug(|msg, src, pos| { println!("[DEBUG] {} ({:?} {:?})", msg, src, pos); });
        engine.register_fn("exec", |cmd: &str| {
            #[cfg(target_os = "windows")] let _ = Command::new("cmd").args(["/C", cmd]).spawn();
            #[cfg(not(target_os = "windows"))] let _ = Command::new("sh").arg("-c").arg(cmd).spawn();
        });
        Self { engine }
    }

    // --- 1. OPTIMISATION : Préparer le contexte UNE FOIS ---
    pub fn build_context(&self, context: &HashMap<String, String>) -> ScriptContext {
        let mut scope = Scope::new();
        self.inject_context(&mut scope, context);
        ScriptContext { scope }
    }

    // --- 2. OPTIMISATION : Exécuter avec contexte existant ---
    pub fn eval_with_context(&self, expr: &str, ctx: &mut ScriptContext, local_value: Option<&str>) -> Option<String> {
        // A. On empile la valeur locale temporairement
        let rewind_count = if let Some(val) = local_value {
            // On parse intelligemment pour que "value > 10" fonctionne
            if let Ok(num) = val.parse::<f64>() {
                ctx.scope.push("value", num);
            } else if let Ok(b) = val.parse::<bool>() {
                ctx.scope.push("value", b);
            } else {
                ctx.scope.push("value", val.to_string());
            }
            1 // On a ajouté 1 variable
        } else {
            0
        };

        // B. On évalue
        let result = if let Ok(res) = self.engine.eval_with_scope::<i64>(&mut ctx.scope, expr) {
            Some(res.to_string())
        } else if let Ok(res) = self.engine.eval_with_scope::<f64>(&mut ctx.scope, expr) {
            Some(res.to_string())
        } else if let Ok(res) = self.engine.eval_with_scope::<bool>(&mut ctx.scope, expr) {
             Some(res.to_string())
        } else if let Ok(res) = self.engine.eval_with_scope::<String>(&mut ctx.scope, expr) {
            Some(res)
        } else {
            None
        };

        // C. On nettoie (Rewind) pour ne pas polluer le scope pour le prochain widget
        ctx.scope.rewind(ctx.scope.len() - rewind_count);
        
        result
    }

    // Gardé pour compatibilité (Actions boutons)
    pub fn run_action(&self, script: &str, context: &HashMap<String, String>) {
        let mut scope = Scope::new();
        self.inject_context(&mut scope, context);
        let _ = self.engine.run_with_scope(&mut scope, script);
    }

    // Validation
    pub fn validate(&self, value: &str, rule: &str) -> bool {
        let mut scope = Scope::new();
        if let Ok(num) = value.parse::<f64>() { scope.push("value", num); } 
        else { scope.push("value", value.to_string()); }
        self.engine.eval_with_scope::<bool>(&mut scope, rule).unwrap_or(false)
    }

    fn inject_context(&self, scope: &mut Scope, context: &HashMap<String, String>) {
        for (k, v) in context {
            // Parsing automatique (int, float, bool, string)
            if let Ok(i) = v.parse::<i64>() { scope.push(k, i); }
            else if let Ok(f) = v.parse::<f64>() { scope.push(k, f); }
            else if let Ok(b) = v.parse::<bool>() { scope.push(k, b); }
            else { scope.push(k, v.clone()); }
        }
    }
}
