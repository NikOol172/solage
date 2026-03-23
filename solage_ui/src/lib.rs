use egui::{Ui, Color32, RichText, Visuals, TextStyle};
use egui_extras::{TableBuilder, Column};
use egui_file_dialog::FileDialog;
use std::fs;
use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet}; // Added HashSet for rehydration
use std::sync::mpsc::Receiver;
use rhai::{Engine, Scope, Map, Dynamic};
use std::sync::Mutex;

use solage_data::{AppConfig, NavState, WidgetDef, Flavor, Step, AppState, GlobalPreferences, WidgetType as SolageWidget}; 
use solage_core::{ScriptEngine, PlatformBackend, ScriptContext, AuthProvider, AuthState, load_config, load_state, save_state, save_preferences, load_preferences};

// ============================================================================
// ANDROID JNI BRIDGE & ASYNC QUEUES
// ============================================================================

/// Queue for pending text inputs received from Android's native UI
pub static PENDING_TEXT_INPUTS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Callback triggered by Android when the user confirms text input
#[cfg(target_os = "android")]
#[no_mangle]
// Using "C" ABI is strictly required for the JVM to find the function signature
pub extern "C" fn Java_com_cloudcompositing_solage_MainActivity_onTextInputReceived(
    mut env: jni::JNIEnv,
    _this: jni::objects::JObject, // Must remain JObject (instance), not JClass!
    j_row_key: jni::objects::JString,
    j_text: jni::objects::JString,
) {
    let row_key: String = match env.get_string(&j_row_key) {
        Ok(java_str) => java_str.into(),
        Err(_) => String::new(),
    };

    let text: String = match env.get_string(&j_text) {
        Ok(java_str) => java_str.into(),
        Err(_) => String::new(),
    };

    if let Ok(mut queue) = PENDING_TEXT_INPUTS.lock() {
        queue.push((row_key, text));
    }
}

/// Requests a native text input dialog on Android
#[cfg(target_os = "android")]
pub fn request_android_text_input(current_text: &str, row_key: &str) {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm();
    let context_ptr = ctx.context();

    if vm_ptr.is_null() || context_ptr.is_null() { return; }

    unsafe {
        if let Ok(jvm) = jni::JavaVM::from_raw(vm_ptr.cast()) {
            
            // CRITICAL FIX: We retrieve the environment without triggering 
            // the automatic thread detachment at the end of the function.
            let mut env = match jvm.get_env() {
                Ok(e) => e,
                Err(_) => jvm.attach_current_thread_permanently().unwrap(),
            };

            let activity = jni::objects::JObject::from_raw(context_ptr.cast());
            
            // Ensure string creation does not panic
            if let (Ok(j_text), Ok(j_key)) = (env.new_string(current_text), env.new_string(row_key)) {
                let _ = env.call_method(
                    &activity,
                    "requestTextInput", // Ensure you update this method name in your Kotlin code!
                    "(Ljava/lang/String;Ljava/lang/String;)V",
                    &[(&j_text).into(), (&j_key).into()]
                );
            }
        }
    }
}

/// Queue for pending file selections from Android's native file picker
pub static PENDING_FILE_SELECTIONS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Callback triggered by Android when a file is selected
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_com_cloudcompositing_solage_MainActivity_onFileSelected(
    mut env: jni::JNIEnv,
    _this: jni::objects::JObject,
    j_row_key: jni::objects::JString,
    j_uri: jni::objects::JString,
) {
    let row_key: String = env.get_string(&j_row_key).map(|s| s.into()).unwrap_or_default();
    let uri: String = env.get_string(&j_uri).map(|s| s.into()).unwrap_or_default();

    if let Ok(mut queue) = crate::PENDING_FILE_SELECTIONS.lock() {
        queue.push((row_key, uri));
    }
}

/// Requests the native file picker on Android
#[cfg(target_os = "android")]
pub fn request_android_file_picker(row_key: &str) {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm();
    let context_ptr = ctx.context();

    if vm_ptr.is_null() || context_ptr.is_null() { return; }

    unsafe {
        if let Ok(jvm) = jni::JavaVM::from_raw(vm_ptr.cast()) {
            let mut env = match jvm.get_env() {
                Ok(e) => e,
                Err(_) => jvm.attach_current_thread_permanently().unwrap(),
            };

            let activity = jni::objects::JObject::from_raw(context_ptr.cast());
            if let Ok(j_key) = env.new_string(row_key) {
                let _ = env.call_method(
                    &activity,
                    "requestFilePicker", // Ensure you update this method name in your Kotlin code!
                    "(Ljava/lang/String;)V",
                    &[(&j_key).into()]
                );
            }
        }
    }
}

// ============================================================================
// APP STRUCTURES & STATE
// ============================================================================

struct LoginForm {
    username: String,
    password: String,
}

impl Default for LoginForm {
    fn default() -> Self {
        Self { username: String::new(), password: String::new() }
    }
}

#[derive(Clone, PartialEq)]
pub enum FileDialogTarget {
    MainConfig,
    WidgetPath(String), // Contains the widget key to update
}

pub struct SolageApp {
    // Core Dependencies
    pub backend: Box<dyn PlatformBackend>,
    pub auth: Box<dyn AuthProvider>,
    pub engine: ScriptEngine,
    
    // Application Data
    pub state: AppState,
    pub preferences: GlobalPreferences,
    pub config: Option<AppConfig>, 
    
    // File & Network IO
    current_config_path: Option<PathBuf>,
    prefs_path: String,
    pub url_input: String,
    pub download_rx: Option<Receiver<Result<String, String>>>,
    pub file_load_rx: Option<Receiver<PathBuf>>,
    pub external_file_rx: Option<std::sync::mpsc::Receiver<(String, String)>>,
    
    // UI State
    error_msg: Option<String>,
    login_form: LoginForm,
    pub toast: Option<(String, f64)>,
    pub theme_applied: bool,
    pub file_dialog: FileDialog,
    pub pending_file_target: Option<FileDialogTarget>,
    pub resolved_values: std::collections::HashMap<String, String>,
}

impl SolageApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>, 
        backend: Box<dyn PlatformBackend>,
        auth: Box<dyn AuthProvider>,
    ) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let prefs_path = backend.get_config_dir().join("user_prefs.json");
        let preferences = load_preferences(prefs_path.to_str().unwrap_or("user_prefs.json"))
            .unwrap_or_default();

        let default_url = backend.default_url().unwrap_or_default();

        Self {
            backend,
            auth,
            engine: ScriptEngine::new(),
            state: AppState::default(),
            preferences,
            config: None,
            current_config_path: None,
            prefs_path: prefs_path.to_string_lossy().to_string(),
            url_input: default_url,
            download_rx: None,
            file_load_rx: None,
            external_file_rx: None,
            error_msg: None,
            login_form: LoginForm::default(),
            toast: None,
            theme_applied: false,
            file_dialog: FileDialog::new(),
            pending_file_target: None,
            resolved_values: HashMap::new()
        }
    }

    // ========================================================================
    // DATA LOADING & IO METHODS
    // ========================================================================

    pub fn load_yaml_string(&mut self, yaml_content: &str) {
        match load_config(yaml_content) {
            Ok(config) => {
                apply_defaults(&config, &mut self.state);
                self.config = Some(config);
            },
            Err(e) => self.error_msg = Some(format!("YAML Parsing Error: {}", e)),
        }
    }

    fn load_config_from_path(&mut self, path: &Path) {
        log::info!("Loading file: {:?}", path);

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                self.error_msg = Some(format!("Failed to read file:\n{}", e));
                return;
            }
        };

        match load_config(&content) {
            Ok(mut cfg) => {
                log::info!("YAML parsed successfully.");
                self.error_msg = None; 

                // 1. Determine private save file path
                let file_stem = path.file_stem().unwrap_or_default().to_string_lossy();
                let json_filename = format!("{}_data.json", file_stem);
                let save_dir = self.backend.get_config_dir();
                let safe_save_path = save_dir.join(&json_filename);

                // 2. Load previously saved user data for this YAML
                if let Ok(loaded_state) = load_state(&safe_save_path.to_string_lossy()) {
                    self.state = loaded_state;
                    self.rehydrate_saved_data(&mut cfg);
                } else {
                    // No existing save found, reset state
                    self.state = AppState::default();
                }
                
                // 3. Store final configuration
                self.config = Some(cfg);
                self.current_config_path = Some(path.to_path_buf());
                self.add_to_recent_files(path.to_path_buf());
            },
            Err(e) => {
                log::error!("YAML Parsing Error: {}", e);
                self.error_msg = Some(format!("YAML Syntax Error:\n{}", e));
            }
        }
    }

    /// Reconstructs dynamic columns (Steps) and populates values from saved JSON data
    /// using absolute paths (Section/Mode/Flavor/Step/Key).
    fn rehydrate_saved_data(&mut self, config: &mut AppConfig) {
        // --------------------------------------------------------------------
        // PASS 1 : Identify and reconstruct missing steps in the correct flavors
        // --------------------------------------------------------------------
        // Maps a flavor path ("Section/Mode/Flavor") to a set of Step names
        let mut missing_steps_map: HashMap<String, HashSet<String>> = HashMap::new();
        
        for key in self.state.user_values.keys() {
            let parts: Vec<&str> = key.split('/').collect();
            // We only process valid absolute paths
            if parts.len() == 5 {
                let flavor_path = format!("{}/{}/{}", parts[0], parts[1], parts[2]);
                let step_name = parts[3].to_string();
                
                missing_steps_map
                    .entry(flavor_path)
                    .or_default()
                    .insert(step_name);
            }
        }

        // Reconstruct steps precisely where they belong in the UI tree
        for section in &mut config.sections {
            for mode in &mut section.modes {
                for flavor in &mut mode.flavors {
                    let flavor_path = format!("{}/{}/{}", section.name, mode.name, flavor.name);
                    
                    if let Some(saved_steps) = missing_steps_map.get(&flavor_path) {
                        // Sort alphabetically so "Step 1" comes before "Step 2"
                        let mut sorted_step_names: Vec<String> = saved_steps.iter().cloned().collect();
                        sorted_step_names.sort(); 

                        for name in sorted_step_names {
                            // If the step doesn't exist in the YAML default, we create it dynamically
                            if !flavor.steps.iter().any(|s| s.name == name) {
                                flavor.steps.push(Step {
                                    name,
                                    values: HashMap::new(),
                                    computes: HashMap::new(),
                                });
                            }
                        }
                    }
                }
            }
        }

        // --------------------------------------------------------------------
        // PASS 2 : Inject the data into the correctly reconstructed tree
        // --------------------------------------------------------------------
        for (saved_key, saved_val) in &self.state.user_values {
            let parts: Vec<&str> = saved_key.split('/').collect();
            if parts.len() == 5 {
                let target_section = parts[0];
                let target_mode = parts[1];
                let target_flavor = parts[2];
                let target_step = parts[3];
                let widget_key = parts[4];

                // Drill down the config tree to find the exact step
                if let Some(section) = config.sections.iter_mut().find(|s| s.name == target_section) {
                    if let Some(mode) = section.modes.iter_mut().find(|m| m.name == target_mode) {
                        if let Some(flavor) = mode.flavors.iter_mut().find(|f| f.name == target_flavor) {
                            if let Some(step) = flavor.steps.iter_mut().find(|s| s.name == target_step) {
                                // Inject the value
                                step.values.insert(widget_key.to_string(), saved_val.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    fn add_to_recent_files(&mut self, path: PathBuf) {
        self.preferences.recent_files.retain(|p| p != &path);
        self.preferences.recent_files.insert(0, path);
        if self.preferences.recent_files.len() > 5 { 
            self.preferences.recent_files.truncate(5); 
        }
        let _ = save_preferences(&self.prefs_path, &self.preferences);
    }

    fn fetch_network_config(&mut self, url: &str, ctx: &egui::Context) {
        self.url_input = url.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        self.download_rx = Some(rx);
        let ctx_clone = ctx.clone();
        
        let request = ehttp::Request::get(url);
        ehttp::fetch(request, move |response| {
            let result = match response {
                Ok(res) if res.ok => Ok(res.text().unwrap_or("").to_string()),
                Ok(res) => Err(format!("Error {}: {}", res.status, res.status_text)),
                Err(e) => Err(format!("Network Error: {}", e)),
            };
            let _ = tx.send(result);
            ctx_clone.request_repaint(); 
        });
    }

    // ========================================================================
    // UI RENDERING METHODS
    // ========================================================================

    fn draw_login_screen(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Login");
                ui.add_space(20.0);
                
                ui.horizontal(|ui| {
                    ui.label("Username:");
                    ui.text_edit_singleline(&mut self.login_form.username);
                });
                ui.horizontal(|ui| {
                    ui.label("Password:");
                    ui.add(egui::TextEdit::singleline(&mut self.login_form.password).password(true));
                });
                
                ui.add_space(10.0);
                
                match self.auth.state() {
                    AuthState::Pending => { ui.spinner(); },
                    AuthState::Failed(msg) => { 
                        ui.colored_label(Color32::RED, msg); 
                    },
                    _ => {}
                }
                
                if ui.button("Login").clicked() {
                    self.auth.login(
                        &self.login_form.username.clone(),
                        &self.login_form.password.clone(),
                        ctx,
                    );
                }
            });
        });
    }

    // ========================================================================
    // DATA MUTATION & SCRIPTING
    // ========================================================================

    pub fn update_user_value(&mut self, absolute_key: &str, new_value: String) {
        self.state.user_values.insert(absolute_key.to_string(), new_value.clone());
        log::debug!("Updated value: {} = {}", absolute_key, new_value);
    }

    pub fn evaluate_compute_rule(script: &str, global_values: &HashMap<String, String>) -> Result<String, String> {
        let engine = rhai::Engine::new();
        let mut scope = rhai::Scope::new();
        
        // 1. On fabrique un véritable dictionnaire dynamique pour Rhai
        let mut rhai_map = rhai::Map::new();
        for (k, v) in global_values {
            // On injecte chaque chemin (ex: "Global/.../width") et sa valeur textuelle
            rhai_map.insert(k.clone().into(), rhai::Dynamic::from(v.clone()));
        }
        
        // 2. On injecte ce dictionnaire dans le moteur sous le nom exact "values"
        scope.push("values", rhai_map);
        
        // 3. On évalue le script avec ce contexte
        match engine.eval_with_scope::<rhai::Dynamic>(&mut scope, script) {
            Ok(result) => Ok(result.to_string()),
            Err(e) => Err(format!("Erreur d'exécution Rhai : {}", e)),
        }
    }

    // ========================================================================
    // ACTIONS & SHORTCUTS
    // ========================================================================

    pub fn save_current_project(&mut self, ctx: &egui::Context) {
        let Some(config) = &self.config else { return; };
        let Some(path) = &self.current_config_path else { return; };

        // 1. Extract all values currently in the UI
        self.state.user_values.clear();
        for section in &config.sections {
            for mode in &section.modes {
                for flavor in &mode.flavors {
                    for step in &flavor.steps {
                        for (key, val) in &step.values {
                            
                            // --- LA MAGIE DES CHEMINS ABSOLUS OPÈRE ICI ---
                            // On crée une adresse unique de type URI / DAG Path
                            // Ex: "Assets/Modeling/Prop/Step 1/width"
                            let absolute_key = format!(
                                "{}/{}/{}/{}/{}", 
                                section.name, 
                                mode.name, 
                                flavor.name, 
                                step.name, 
                                key
                            );
                            
                            self.state.user_values.insert(absolute_key, val.clone());
                        }
                    }
                }
            }
        }

        // 2. Determine safe save path
        let path_str = path.to_string_lossy().to_string();
        let state_path = if path_str.starts_with("http") {
            path_str 
        } else {
            let file_stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let json_filename = format!("{}_data.json", file_stem);
            
            let save_dir = self.backend.get_config_dir();
            if let Err(e) = std::fs::create_dir_all(&save_dir) {
                log::error!("Failed to create config directory: {}", e);
            }
            save_dir.join(&json_filename).to_string_lossy().to_string()
        };

        // 3. Save state
        if let Err(e) = save_state(&state_path, &self.state) {
            self.error_msg = Some(format!("Save Error: {}", e));
        } else {
            self.toast = Some(("✅ Project Saved".to_string(), ctx.input(|i| i.time)));
            log::info!("Successfully saved to: {}", state_path);
        }
    }

    fn handle_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            self.save_current_project(ctx);
        }
    }

    // ========================================================================
    // ASYNC EVENT ROUTER
    // ========================================================================

    fn handle_async_events(&mut self, ctx: &egui::Context) {
        // NOUVEAU : Le drapeau pour avertir l'interface qu'une donnée asynchrone est arrivée
        let mut needs_sync = false;

        // --- 1. Desktop File Dialog ---
        #[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
        {
            self.file_dialog.update(ctx);
            if let Some(path) = self.file_dialog.take_picked() {
                if let Some(target) = self.pending_file_target.clone() {
                    match target {
                        FileDialogTarget::MainConfig => self.load_config_from_path(&path),
                        FileDialogTarget::WidgetPath(key) => {
                            let path_str = path.display().to_string();
                            self.update_user_value(&key, path_str);
                            needs_sync = true; // <-- ON LÈVE LE DRAPEAU
                        }
                    }
                }
                self.pending_file_target = None;
            }
        }

        // --- 2. External File Receiver (Web / Android) ---
        #[cfg(any(target_arch = "wasm32", target_os = "android"))]
        if let Some(rx) = &self.external_file_rx {
            if let Ok((file_name, content)) = rx.try_recv() {
                self.load_yaml_string(&content);
                
                let save_dir = self.backend.get_config_dir();
                let _ = std::fs::create_dir_all(&save_dir);
                
                let safe_file_name = std::path::Path::new(&file_name)
                    .file_name().unwrap_or_default().to_string_lossy().to_string();
                
                let local_yaml_path = save_dir.join(&safe_file_name);
                
                if let Err(e) = std::fs::write(&local_yaml_path, &content) {
                    log::error!("Local YAML copy error: {}", e);
                }
                
                self.current_config_path = Some(local_yaml_path.clone());
                self.add_to_recent_files(local_yaml_path);
                
                let file_stem = std::path::Path::new(&safe_file_name).file_stem().unwrap_or_default().to_string_lossy();
                let json_filename = format!("{}_data.json", file_stem);
                let safe_save_path = save_dir.join(&json_filename);

                if let Ok(saved_state) = load_state(&safe_save_path.to_string_lossy()) {
                    self.state = saved_state;
                    if let Some(mut cfg) = self.config.take() {
                        self.rehydrate_saved_data(&mut cfg);
                        self.config = Some(cfg);
                    }
                } else {
                    self.state = AppState::default();
                }

                self.external_file_rx = None;
            }
        }

        // --- 3. Network Download Receiver ---
        if let Some(rx) = &self.download_rx {
            if let Ok(result) = rx.try_recv() {
                self.download_rx = None; 
                match result {
                    Ok(yaml) => {
                        self.load_yaml_string(&yaml);
                        if self.error_msg.is_none() {
                            let url_path = PathBuf::from(&self.url_input);
                            self.add_to_recent_files(url_path.clone());
                            self.current_config_path = Some(url_path);
                            
                            if let Ok(saved_state) = load_state(&self.url_input) {
                                self.state = saved_state;
                            }
                        }
                    },
                    Err(e) => self.error_msg = Some(e),
                }
            }
        }

        // --- 4. Local File Load Receiver ---
        if let Some(rx) = &self.file_load_rx {
            if let Ok(path) = rx.try_recv() {
                self.load_config_from_path(&path);
                self.file_load_rx = None; 
            }
        }

        // --- 5. Android Text Input Queue ---
        #[cfg(target_os = "android")]
        if let Ok(mut queue) = PENDING_TEXT_INPUTS.lock() {
            if !queue.is_empty() {
                for (row_key, new_text) in queue.drain(..) {
                    if row_key == "url_input" {
                        self.url_input = new_text;
                    } else {
                        self.update_user_value(&row_key, new_text);
                        needs_sync = true; // <-- ON LÈVE LE DRAPEAU
                    }
                }
            }
        }
        
        // --- 6. Android File Selection Queue ---
        #[cfg(target_os = "android")]
        if let Ok(mut queue) = crate::PENDING_FILE_SELECTIONS.lock() {
            if !queue.is_empty() {
                for (row_key, uri) in queue.drain(..) {
                    self.update_user_value(&row_key, uri);
                    needs_sync = true; // <-- ON LÈVE LE DRAPEAU
                }
            }
        }

        // ====================================================================
        // LA LIGNE MAGIQUE DE SYNCHRONISATION
        // ====================================================================
        if needs_sync {
            // On contourne le Borrow Checker avec "take()" pour sortir temporairement 
            // la configuration, la mettre à jour avec les nouvelles données du "state", 
            // puis la remettre à sa place.
            if let Some(mut cfg) = self.config.take() {
                self.rehydrate_saved_data(&mut cfg);
                self.config = Some(cfg);
            }
        }
    }

    // ========================================================================
    // UI SUB-COMPONENTS
    // ========================================================================

    fn draw_android_safe_areas(&self, ctx: &egui::Context) {
        #[cfg(target_os = "android")]
        egui::TopBottomPanel::top("android_safe_area_top")
            .frame(egui::Frame::none()).exact_height(45.0).show(ctx, |_ui| {});

        #[cfg(target_os = "android")]
        egui::TopBottomPanel::bottom("android_safe_area_bottom")
            .frame(egui::Frame::none()).exact_height(35.0).show(ctx, |_ui| {});
    }

    fn draw_welcome_screen(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(egui::RichText::new("🛠️ BUILD: Solage Engine v1.0").color(egui::Color32::GRAY).size(12.0));
            
            if let Some(err) = &self.error_msg {
                ui.group(|ui| {
                    ui.colored_label(Color32::RED, "🛑 Error");
                    ui.monospace(err); 
                });
                ui.add_space(20.0);
            }

            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.heading(RichText::new("SOLAGE").size(60.0).strong().color(Color32::from_rgb(100, 180, 255)));
                ui.label(RichText::new("VFX Pipeline Manager").size(20.0).color(Color32::GRAY));
            });
            ui.add_space(40.0);

            ui.vertical_centered(|ui| {
                let btn = egui::Button::new(egui::RichText::new("📂 Open local file...").size(16.0))
                    .min_size(egui::vec2(300.0, 35.0));

                if ui.add(btn).clicked() {
                    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
                    {
                        self.pending_file_target = Some(FileDialogTarget::MainConfig);
                        self.file_dialog.pick_file(); 
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        let (tx, rx) = std::sync::mpsc::channel();
                        self.external_file_rx = Some(rx);
                        wasm_bindgen_futures::spawn_local(async move {
                            if let Some(file) = rfd::AsyncFileDialog::new().pick_file().await {
                                let bytes = file.read().await;
                                if let Ok(text) = String::from_utf8(bytes) {
                                    let _ = tx.send((file.file_name(), text));
                                }
                            }
                        });
                    }
                    #[cfg(target_os = "android")]
                    {
                        let (tx, rx) = std::sync::mpsc::channel();
                        self.external_file_rx = Some(rx);
                        self.backend.pick_file_async_mobile(tx);
                    }
                }
                
                ui.add_space(15.0);

                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    ui.horizontal(|ui| {
                        let response = ui.add(egui::TextEdit::singleline(&mut self.url_input).desired_width(220.0));
                        
                        #[cfg(target_os = "android")]
                        if response.clicked() {
                            crate::request_android_text_input(&self.url_input, "url_input");
                        }

                        if ui.add(egui::Button::new("⬇ URL").min_size(egui::vec2(70.0, 30.0))).clicked() {
                            let url = self.url_input.clone();
                            self.fetch_network_config(&url, ctx);
                        }
                    });
                });

                if !self.preferences.recent_files.is_empty() {
                    ui.add_space(30.0);
                    ui.separator();
                    ui.add_space(10.0);
                    ui.label(RichText::new("RECENTLY OPENED").size(12.0).strong().color(Color32::GRAY));
                    ui.add_space(10.0);

                    let recent_files = self.preferences.recent_files.clone();
                    for path in recent_files {
                        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                        let full_path = path.to_string_lossy();
                        
                        ui.scope(|ui| {
                            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = Color32::from_gray(30);
                            let btn = egui::Button::new(RichText::new(format!("📄 {}", file_name)).size(14.0))
                                .min_size(egui::vec2(280.0, 28.0))
                                .frame(true);

                            if ui.add(btn).on_hover_text(full_path.to_string()).clicked() {
                                let path_str = full_path.to_string();
                                if path_str.starts_with("http") {
                                    self.fetch_network_config(&path_str, ctx);
                                } else {
                                    self.load_config_from_path(&path.clone());
                                }
                            }
                        });
                        ui.add_space(5.0);
                    }
                }
            });
        });
    }

    fn draw_toast_notifications(&mut self, ctx: &egui::Context) {
        if let Some((msg, start_time)) = &self.toast {
            let current_time = ctx.input(|i| i.time);
            let elapsed = current_time - start_time;
            
            if elapsed < 2.5 {
                let alpha = if elapsed > 2.0 { 1.0 - ((elapsed - 2.0) * 2.0) as f32 } else { 1.0 };
                
                egui::Area::new(egui::Id::new("toast_area"))
                    .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style())
                            .fill(Color32::from_black_alpha((200.0 * alpha) as u8))
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(8.0)
                            .inner_margin(12.0)
                            .show(ui, |ui| {
                                ui.label(RichText::new(msg).color(Color32::from_white_alpha((255.0 * alpha) as u8)).strong().size(16.0));
                            });
                    });
                ctx.request_repaint();
            } else {
                self.toast = None;
            }
        }
    }

    // ========================================================================
    // MOTEUR DE CALCUL RÉACTIF
    // ========================================================================
    pub fn recalculate_all_compute_rules(&mut self) {
        let Some(config) = &mut self.config else { return; };

        // 1. On construit le dictionnaire brut avec les valeurs littérales (y compris les "=...")
        let mut raw_values = HashMap::new();
        for section in &config.sections {
            for mode in &section.modes {
                for flavor in &mode.flavors {
                    for step in &flavor.steps {
                        for (key, val) in &step.values {
                            let abs_key = format!("{}/{}/{}/{}/{}", section.name, mode.name, flavor.name, step.name, key);
                            raw_values.insert(abs_key, val.clone());
                        }
                    }
                }
            }
        }

        // 2. RÉSOLUTION DES LIENS DYNAMIQUES (Le parseur de références)
        let mut global_values = raw_values.clone();
        
        for (abs_key, val) in global_values.iter_mut() {
            if val.starts_with('=') {
                let mut current_val = val.clone();
                let mut visited = std::collections::HashSet::new();
                visited.insert(abs_key.clone()); // On mémorise le point de départ

                // On remonte la chaîne tant qu'on trouve un signe '='
                while let Some(target) = current_val.strip_prefix('=') {
                    let target_str = target.to_string();
                    
                    // Sécurité anti-crash : détection de boucle infinie (A -> B -> A)
                    if visited.contains(&target_str) {
                        log::error!("Boucle infinie détectée sur la clé : {}", abs_key);
                        current_val = "0".to_string();
                        break;
                    }
                    visited.insert(target_str.clone());

                    // On cherche la valeur cible dans le dictionnaire brut
                    if let Some(target_val) = raw_values.get(&target_str) {
                        current_val = target_val.clone();
                    } else {
                        // Le lien pointe vers une case qui n'existe pas ou a été supprimée
                        current_val = "0".to_string(); 
                        break;
                    }
                }
                
                // On écrase la formule par sa valeur finale résolue pour Rhai
                *val = current_val;
            }
        }

        // 3. On passe sur toutes les règles de calcul Rhai
        for section in &mut config.sections {
            for mode in &mut section.modes {
                for flavor in &mut mode.flavors {
                    for step in &mut flavor.steps {
                        for row_def in &flavor.row_definitions {
                            // LA MAGIE EST ICI : On cherche d'abord dans le Step, sinon dans le RowDef
                            let active_script = step.computes.get(&row_def.key)
                                .or(row_def.widget.compute.as_ref());

                            if let Some(script) = active_script {
                                match SolageApp::evaluate_compute_rule(script, &global_values) {
                                    Ok(result) => {
                                        step.values.insert(row_def.key.clone(), result.clone());
                                        let abs_key = format!("{}/{}/{}/{}/{}", section.name, mode.name, flavor.name, step.name, row_def.key);
                                        global_values.insert(abs_key, result);
                                    }
                                    Err(e) => {
                                        println!("🛑 ERREUR RHAI pour '{}': {}", row_def.key, e);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        self.resolved_values = global_values;

    }
}

impl eframe::App for SolageApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let is_mobile = ctx.content_rect().width() < 600.0;

        // 1. Core Event Handling
        self.handle_async_events(ctx);
        self.draw_android_safe_areas(ctx);

        if !self.theme_applied {
            // Assurez-vous d'avoir renommé apply_studio_theme ou laissez l'ancien nom si conservé ailleurs
            apply_studio_theme(ctx);
            self.theme_applied = true;
        }

        // 2. Authentication Gate
        self.auth.poll();
        if !self.auth.is_ready() {
            self.draw_login_screen(ctx);
            return;
        }

        // 3. Splash Screen Rendering (Desktop Initial Load)
        #[cfg(not(target_os = "android"))] 
        {
            let time = ctx.input(|i| i.time);
            if time < 2.5 {
                if time < 1.5 {
                    draw_splash_anim(ctx, time); // À renommer si vous traduisez aussi ce fichier
                    ctx.request_repaint();
                    return; 
                }
            }
        }

        // 4. Main Router
        let show_welcome_screen = self.config.as_ref().map_or(true, |cfg| cfg.sections.is_empty());

        if show_welcome_screen {
            self.draw_welcome_screen(ctx);
        } else {
            self.handle_keyboard_shortcuts(ctx);
            
            // 1. LES DRAPEAUX D'ACTION
            let mut action_save = false;
            let mut action_close = false;
            let mut action_compute = false;
            
            // 2. LE BLOC D'EMPRUNT (Scope)
            // On enferme la lecture de `config` et le dessin de l'interface dans un bloc { }
            {
                let config = self.config.as_mut().unwrap();

                // Build Rhai global context with Absolute Paths
                let mut global_values = HashMap::new();
                for section in &config.sections {
                    for mode in &section.modes {
                        for flavor in &mode.flavors {
                            for step in &flavor.steps {
                                for (key, val) in &step.values {
                                    // On génère la même adresse unique (URI) que pour la sauvegarde
                                    let absolute_key = format!(
                                        "{}/{}/{}/{}/{}", 
                                        section.name, 
                                        mode.name, 
                                        flavor.name, 
                                        step.name, 
                                        key
                                    );
                                    
                                    // Le moteur Rhai a maintenant accès à l'arbre complet !
                                    global_values.insert(absolute_key, val.clone());
                                }
                            }
                        }
                    }
                }
                let mut script_context = self.engine.build_context(&global_values);

                // --- A. Top Panel ---
                egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Solage");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            
                            // On lève les drapeaux au lieu de bloquer l'application !
                            if ui.button("Close Project").clicked() {
                                action_save = true;
                                action_close = true;
                            }

                            if ui.button("💾 Save").clicked() {
                                action_save = true;
                            }

                            ui.label(format!("{} sections", config.sections.len()));
                        });
                    });
                });

                // --- B. Bottom Panel ---
                egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Actions:");
                        for action in &config.actions {
                            if ui.button(&action.label).clicked() {
                                self.engine.run_action(&action.script, &global_values);
                            }
                        }
                    });
                });

                // --- C. Navigation ---
                if is_mobile {
                    egui::TopBottomPanel::bottom("mobile_nav").show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            let available = ui.available_width();
                            let btn_width = available / config.sections.len() as f32;
                            for (idx, section) in config.sections.iter().enumerate() {
                                let is_selected = self.state.nav.section == idx;
                                let label = format!("{}\n{}", section.icon, section.name);
                                if ui.add_sized([btn_width, 50.0], egui::Button::selectable(is_selected, RichText::new(&label).size(11.0))).clicked() {
                                    self.state.nav.section = idx;
                                    self.state.nav.mode = 0;
                                    self.state.nav.flavor = 0;
                                }
                            }
                        });
                    });
                } else {
                    egui::SidePanel::left("sidebar").show(ctx, |ui| {
                        ui.add_space(10.0);
                        for (idx, section) in config.sections.iter().enumerate() {
                            let is_selected = self.state.nav.section == idx;
                            let label = format!("{} {}", section.icon, section.name);
                            if ui.selectable_label(is_selected, label).clicked() {
                                self.state.nav.section = idx;
                                self.state.nav.mode = 0;
                                self.state.nav.flavor = 0;
                            }
                        }
                        ui.add_space(20.0);
                        if ui.button("📂 Open other...").clicked() {
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                self.pending_file_target = Some(FileDialogTarget::MainConfig);
                                self.file_dialog.pick_file(); 
                            }
                            #[cfg(target_arch = "wasm32")]
                            {
                                let (tx, rx) = std::sync::mpsc::channel();
                                self.external_file_rx = Some(rx);
                                wasm_bindgen_futures::spawn_local(async move {
                                    if let Some(file) = rfd::AsyncFileDialog::new().pick_file().await {
                                        let bytes = file.read().await;
                                        if let Ok(text) = String::from_utf8(bytes) {
                                            let _ = tx.send((file.file_name(), text));
                                        }
                                    }
                                });
                            }
                        }
                    });
                }

                // --- D. Central Panel ---
                egui::CentralPanel::default().show(ctx, |ui| {
                    if let Some(err) = &self.error_msg {
                        ui.colored_label(Color32::RED, format!("⚠ {}", err));
                    }

                    let section_idx = self.state.nav.section;
                    if let Some(active_section) = config.sections.get_mut(section_idx) {
                        ui.horizontal(|ui| {
                            for (mode_idx, mode) in active_section.modes.iter().enumerate() {
                                let is_selected = self.state.nav.mode == mode_idx;
                                if ui.selectable_label(is_selected, &mode.name).clicked() {
                                    self.state.nav.mode = mode_idx;
                                    self.state.nav.flavor = 0;
                                }
                            }
                        });
                        ui.separator();

                        let mode_idx = self.state.nav.mode;
                        if let Some(active_mode) = active_section.modes.get_mut(mode_idx) {
                            ui.horizontal(|ui| {
                                ui.label("Flavor:");
                                let flavor_idx = self.state.nav.flavor;
                                let current_name = active_mode.flavors.get(flavor_idx).map(|f| f.name.clone()).unwrap_or_default();
                                
                                egui::ComboBox::from_id_salt("flavor_cb")
                                    .selected_text(current_name)
                                    .show_ui(ui, |ui| {
                                        for (i, f) in active_mode.flavors.iter().enumerate() {
                                            ui.selectable_value(&mut self.state.nav.flavor, i, &f.name);
                                        }
                                    });

                                if let Some(active_flavor) = active_mode.flavors.get_mut(self.state.nav.flavor) {
                                    ui.add_space(15.0); // Un petit espace visuel
                                    
                                    if ui.button("➕ Add Step").clicked() {
                                        let new_index = active_flavor.steps.len() + 1;
                                        active_flavor.steps.push(Step {
                                            name: format!("Step {}", new_index),
                                            values: std::collections::HashMap::new(),
                                            computes: std::collections::HashMap::new(),
                                        });
                                    }

                                }
                            }); 
                            
                            let flavor_idx = self.state.nav.flavor;
                            if let Some(active_flavor) = active_mode.flavors.get_mut(flavor_idx) {
                                let base_path = format!("{}/{}/{}", active_section.name, active_mode.name, active_flavor.name);
                                
                                if is_mobile {
                                    action_compute = draw_single_step(ui, active_flavor, &base_path, &mut script_context, &self.engine, &mut self.file_dialog, &mut self.pending_file_target, &mut self.state.nav, &self.resolved_values);
                                } else {
                                    action_compute = draw_comparison_table(ui, active_flavor, &base_path, &mut script_context, &self.engine, &mut self.file_dialog, &mut self.pending_file_target, &self.resolved_values);
                                }
                            }
                        }
                    }

                });
                
            } // <-- 3. L'accolade se ferme ici ! L'écran est dessiné, `config` est libéré.

            // 4. L'EXÉCUTION DES DRAPEAUX
            // Maintenant que nous avons l'accès exclusif à `self`, on peut appeler nos méthodes.
            if action_compute {
                println!("action compute '{}'", action_compute);
                self.recalculate_all_compute_rules();
            }
            if action_save {
                self.save_current_project(ctx);
            }
            if action_close {
                if let Some(cfg) = &mut self.config {
                    cfg.sections.clear();
                }
                self.current_config_path = None;
            }
        }

        // 5. Overlays (Toasts & Ending Splash)
        self.draw_toast_notifications(ctx);

        #[cfg(not(target_os = "android"))]
        {
            let time = ctx.input(|i| i.time);
            if time >= 1.5 && time < 2.5 {
                draw_splash_anim(ctx, time);
                ctx.request_repaint();
            }
        }
    }
}

// ============================================================================
// UI RENDERING HELPERS
// ============================================================================

fn draw_splash_anim(ctx: &egui::Context, time: f64) {
    let fade_out = if time > 1.5 { (2.5 - time).clamp(0.0, 1.0) as f32 } else { 1.0 };
    
    egui::CentralPanel::default().show(ctx, |ui| {
        let rect = ui.max_rect();
        ui.painter().rect_filled(
            rect, 
            egui::CornerRadius::ZERO, 
            egui::Color32::from_black_alpha((255.0 * fade_out) as u8)
        );

        let center = rect.center();
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            "SOLAGE",
            egui::FontId::proportional(80.0),
            egui::Color32::from_white_alpha((255.0 * fade_out) as u8),
        );
        
        let w = 200.0;
        let h = 4.0;
        let progress = (time / 1.5).clamp(0.0, 1.0) as f32;
        let bar_rect = egui::Rect::from_center_size(center + egui::vec2(0.0, 60.0), egui::vec2(w, h));
        
        ui.painter().rect_filled(bar_rect, 2.0, Color32::from_gray(60).gamma_multiply(fade_out));
        ui.painter().rect_filled(
            egui::Rect::from_min_size(bar_rect.min, egui::vec2(w * progress, h)), 
            2.0, 
            Color32::LIGHT_BLUE.gamma_multiply(fade_out)
        );
    });
}

fn draw_single_step(
    ui: &mut Ui,
    flavor: &mut Flavor,
    base_path: &str,
    _script_context: &mut ScriptContext,
    engine: &ScriptEngine,
    file_dialog: &mut egui_file_dialog::FileDialog,
    pending_target: &mut Option<FileDialogTarget>,
    nav: &mut NavState,
    resolved_values: &std::collections::HashMap<String, String>,
) -> bool {
    let mut ui_changed = false;
    
    // 1. LA DÉCLARATION EST ICI AUSSI
    let mut step_to_remove: Option<usize> = None;
    
    if flavor.steps.is_empty() {
        ui.colored_label(Color32::GRAY, "No steps defined");
        return false;
    }

    let step_count = flavor.steps.len();
    let current = nav.step.min(step_count.saturating_sub(1));

    ui.horizontal(|ui| {
        if ui.button("◀").clicked() && current > 0 { nav.step -= 1; }
        
        ui.strong(RichText::new(&flavor.steps[current].name).size(16.0).color(Color32::from_rgb(100, 180, 255)));
        
        // 2. LE MENU CONTEXTUEL
        ui.menu_button("...", |ui| {
            if ui.button("X Remove Step").clicked() {
                step_to_remove = Some(current);
                ui.close_menu();
            }
        });

        if ui.button("▶").clicked() && current < step_count - 1 { nav.step += 1; }
        ui.label(format!("{}/{}", current + 1, step_count));
    });
    ui.separator();

    let step = &mut flavor.steps[current];
    for row_def in &flavor.row_definitions {
        
        // On récupère la valeur existante ou on l'initialise
        let value = step.values.entry(row_def.key.clone()).or_insert_with(|| {
            row_def.widget.default.as_ref().map(|d| match d {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => String::new(),
            }).unwrap_or_default()
        });
            
        let absolute_key = format!("{}/{}/{}", base_path, step.name, row_def.key);
        let mut cell_changed = false;

        let row_response = ui.horizontal(|ui| {
            ui.set_min_width(ui.available_width());
            
            // --- A. LA SOURCE DU GLISSEMENT ---
            let handle = ui.add(egui::Label::new("⠿").sense(egui::Sense::drag()));
            if handle.dragged() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
            }
            handle.dnd_set_drag_payload(std::sync::Arc::new(value.clone()));
            
            ui.label(&row_def.label);

            // --- L'INTERCEPTION INTELLIGENTE ---
            let is_reference = value.starts_with('=');
            let is_local_compute = step.computes.contains_key(&row_def.key);
            let has_global_compute = row_def.widget.compute.is_some();
            let has_compute = is_local_compute || has_global_compute;

            if is_reference || has_compute {
                let display_val = if is_reference {
                    resolved_values.get(&absolute_key).cloned().unwrap_or_else(|| "0".to_string())
                } else {
                    value.clone()
                };

                if is_reference {
                    if ui.button("🔗").on_hover_text("Cliquer pour casser le lien (Bake)").clicked() {
                        *value = display_val.clone();
                        cell_changed = true;
                    }
                } else {
                    // CHANGEMENT D'ICÔNE : 🛠 si c'est local, ⚙ si c'est global
                    let icon = if is_local_compute { "🛠" } else { "⚙" };
                    
                    ui.menu_button(icon, |ui| {
                        if is_local_compute {
                            ui.strong("Script Local (Surcharge)");
                        } else {
                            ui.label("Script Global (Hérité)");
                        }
                        
                        // On récupère le texte à éditer (priorité locale)
                        let mut script_to_edit = step.computes.get(&row_def.key)
                            .cloned()
                            .or_else(|| row_def.widget.compute.clone())
                            .unwrap_or_default();

                        // L'éditeur texte
                        if ui.add(egui::TextEdit::multiline(&mut script_to_edit)
                            .desired_width(250.0)
                            .font(egui::TextStyle::Monospace))
                            .changed() 
                        {
                            // Dès qu'on tape une lettre, on l'enregistre dans le STEP (Surcharge locale !)
                            step.computes.insert(row_def.key.clone(), script_to_edit);
                            cell_changed = true;
                        }
                        
                        ui.separator();
                        
                        if is_local_compute {
                            // Bouton pour détruire la surcharge et revenir au global
                            if ui.button("🗑 Supprimer la surcharge locale").clicked() {
                                step.computes.remove(&row_def.key);
                                cell_changed = true;
                                ui.close_menu();
                            }
                        }
                    }).response.on_hover_text(if is_local_compute { "Modifier la surcharge" } else { "Créer une surcharge locale" });
                }

                let tooltip_text = if is_reference { format!("Lié à : {}", value) } else { "Calcul dynamique".to_string() };
                ui.strong(egui::RichText::new(&display_val).color(egui::Color32::from_rgb(100, 180, 255)))
                    .on_hover_text(tooltip_text);
                    
            } else {
                cell_changed |= draw_cell_value(ui, value, &row_def.widget, &absolute_key, engine, file_dialog, pending_target);
            }
        });

        // --- B. LA ZONE DE RÉCEPTION ---
        let drop_zone = row_response.response;

        if drop_zone.dnd_hover_payload::<std::sync::Arc<String>>().is_some() {
            ui.painter().rect_stroke(
                drop_zone.rect.expand(2.0),
                2.0,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 255, 100)),
                egui::StrokeKind::Inside,
            );
        }

        if let Some(dropped_value) = drop_zone.dnd_release_payload::<std::sync::Arc<String>>() {
            *value = dropped_value.to_string();
            cell_changed = true;
        }

        if cell_changed {
            ui_changed = true;
        }
    }

    // 3. LA SUPPRESSION : Tout en bas
    if let Some(idx) = step_to_remove {
        flavor.steps.remove(idx);
        if nav.step >= flavor.steps.len() && nav.step > 0 {
            nav.step -= 1;
        }
        ui_changed = true;
    }

    ui_changed
}

fn draw_comparison_table(
    ui: &mut Ui, 
    flavor: &mut Flavor, 
    base_path: &str, 
    _script_context: &mut ScriptContext, 
    engine: &ScriptEngine, 
    file_dialog: &mut egui_file_dialog::FileDialog,
    pending_target: &mut Option<FileDialogTarget>,
    resolved_values: &std::collections::HashMap<String, String>,
) -> bool {
    let mut ui_changed = false;
    
    // 1. LA DÉCLARATION EST ICI : Tout en haut, visible par toute la fonction !
    let mut step_to_remove: Option<usize> = None;
    let mut step_to_move: Option<(usize, usize)> = None;

    if flavor.steps.is_empty() {
        ui.colored_label(Color32::GRAY, "No steps defined");
        return false;
    }

    let mut builder = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .column(Column::initial(150.0));

    for _ in &flavor.steps {
        builder = builder.column(Column::remainder());
    }

    builder
        .header(30.0, |mut header| {
            header.col(|ui| { 
                ui.strong(RichText::new("Parameter").color(Color32::WHITE)); 
            });
            
            for (step_idx, step) in flavor.steps.iter().enumerate() {
                header.col(|ui| { 
                    
                    // 1. On encapsule tout le titre dans une "response" pour en faire une zone de Drop
                    let response = ui.horizontal(|ui| {
                        
                        // A. La poignée de Drag (Pour DÉPLACER LA COLONNE)
                        let handle = ui.add(egui::Label::new(" :: ").sense(egui::Sense::drag()))
                            .on_hover_cursor(egui::CursorIcon::Grab)
                            .on_hover_text("Glisser pour déplacer la colonne"); // Texte corrigé !

                        if handle.dragged() {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
                        }

                        // LA CORRECTION EST ICI : Le payload est juste l'index de la colonne (step_idx)
                        handle.dnd_set_drag_payload(std::sync::Arc::new(step_idx));

                        // B. Le Titre et le Menu (Intacts)
                        ui.strong(RichText::new(&step.name).color(Color32::from_rgb(100, 180, 255)));
                        ui.menu_button("...", |ui| {
                            if ui.button("X Remove Step").clicked() {
                                step_to_remove = Some(step_idx);
                                ui.close_menu();
                            }
                        });
                    }).response;

                    // 2. Feedback Visuel (Orange pour différencier du Vert/Bleu des cellules)
                    if response.dnd_hover_payload::<std::sync::Arc<usize>>().is_some() {
                        ui.painter().rect_stroke(
                            response.rect.expand(2.0),
                            2.0,
                            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 150, 50)), // Orange
                            egui::StrokeKind::Inside,
                        );
                    }

                    // 3. Réception du Drop (Réagencement des colonnes)
                    if let Some(source_idx) = response.dnd_release_payload::<std::sync::Arc<usize>>() {
                        // On enregistre (De, Vers) pour faire le mouvement à la fin
                        step_to_move = Some((**source_idx, step_idx));
                    }
                });
            }
        })
        .body(|mut body| {
            for row_def in &flavor.row_definitions {
                body.row(24.0, |mut strip| {
                    
                    // 1. La première colonne : le nom du paramètre (Label)
                    strip.col(|ui| { ui.label(&row_def.label); });

                    // 2. Les colonnes suivantes : les valeurs pour chaque Step
                    for step in &mut flavor.steps {
                        strip.col(|ui| {
                            
                            // A. ON CRÉE LES VARIABLES DANS LE SCOPE DE LA CELLULE
                            let value = step.values.entry(row_def.key.clone()).or_insert_with(|| {
                                row_def.widget.default.as_ref().map(|d| match d {
                                    serde_json::Value::String(s) => s.clone(),
                                    serde_json::Value::Number(n) => n.to_string(),
                                    serde_json::Value::Bool(b) => b.to_string(),
                                    _ => String::new(),
                                }).unwrap_or_default()
                            });

                            let absolute_key = format!("{}/{}/{}", base_path, step.name, row_def.key);
                            let mut cell_changed = false;

                            // B. ON DESSINE LA CELLULE (Poignée + Widget)
                            let cell_response = ui.horizontal(|ui| {
                                let handle = ui.add(egui::Label::new(" :: ").sense(egui::Sense::drag()))
                                    .on_hover_cursor(egui::CursorIcon::Grab)
                                    .on_hover_text("Glisser : Copier | Shift+Glisser : Lier");

                                if handle.dragged() {
                                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
                                }

                                // Ici, les variables existent parfaitement !
                                let payload_data = (absolute_key.clone(), value.clone());
                                handle.dnd_set_drag_payload(std::sync::Arc::new(payload_data));

                                // --- L'INTERCEPTION INTELLIGENTE ---
                                let is_reference = value.starts_with('=');
                                let is_local_compute = step.computes.contains_key(&row_def.key);
                                let has_global_compute = row_def.widget.compute.is_some();
                                let has_compute = is_local_compute || has_global_compute;

                                if is_reference || has_compute {
                                    let display_val = if is_reference {
                                        resolved_values.get(&absolute_key).cloned().unwrap_or_else(|| "0".to_string())
                                    } else {
                                        value.clone()
                                    };

                                    if is_reference {
                                        if ui.button("🔗").on_hover_text("Cliquer pour casser le lien (Bake)").clicked() {
                                            *value = display_val.clone();
                                            cell_changed = true;
                                        }
                                    } else {
                                        // CHANGEMENT D'ICÔNE : 🛠 si c'est local, ⚙ si c'est global
                                        let icon = if is_local_compute { "🛠" } else { "⚙" };
                                        
                                        ui.menu_button(icon, |ui| {
                                            if is_local_compute {
                                                ui.strong("Script Local (Surcharge)");
                                            } else {
                                                ui.label("Script Global (Hérité)");
                                            }
                                            
                                            // On récupère le texte à éditer (priorité locale)
                                            let mut script_to_edit = step.computes.get(&row_def.key)
                                                .cloned()
                                                .or_else(|| row_def.widget.compute.clone())
                                                .unwrap_or_default();

                                            // L'éditeur texte
                                            if ui.add(egui::TextEdit::multiline(&mut script_to_edit)
                                                .desired_width(250.0)
                                                .font(egui::TextStyle::Monospace))
                                                .changed() 
                                            {
                                                // Dès qu'on tape une lettre, on l'enregistre dans le STEP (Surcharge locale !)
                                                step.computes.insert(row_def.key.clone(), script_to_edit);
                                                cell_changed = true;
                                            }
                                            
                                            ui.separator();
                                            
                                            if is_local_compute {
                                                // Bouton pour détruire la surcharge et revenir au global
                                                if ui.button("🗑 Supprimer la surcharge locale").clicked() {
                                                    step.computes.remove(&row_def.key);
                                                    cell_changed = true;
                                                    ui.close_menu();
                                                }
                                            }
                                        }).response.on_hover_text(if is_local_compute { "Modifier la surcharge" } else { "Créer une surcharge locale" });
                                    }

                                    let tooltip_text = if is_reference { format!("Lié à : {}", value) } else { "Calcul dynamique".to_string() };
                                    ui.strong(egui::RichText::new(&display_val).color(egui::Color32::from_rgb(100, 180, 255)))
                                        .on_hover_text(tooltip_text);
                                        
                                } else {
                                    cell_changed |= draw_cell_value(ui, value, &row_def.widget, &absolute_key, engine, file_dialog, pending_target);
                                }
                            });

                            // C. LA LOGIQUE DE DROP (Réception)
                            let drop_zone = cell_response.response;

                            if drop_zone.dnd_hover_payload::<std::sync::Arc<(String, String)>>().is_some() {
                                let is_link = ui.input(|i| i.modifiers.shift);
                                let stroke_color = if is_link { 
                                    egui::Color32::from_rgb(100, 180, 255) // Bleu pour Lier
                                } else { 
                                    egui::Color32::from_rgb(100, 255, 100) // Vert pour Copier
                                };
                                
                                ui.painter().rect_stroke(
                                    drop_zone.rect.expand(2.0),
                                    2.0,
                                    egui::Stroke::new(2.0, stroke_color),
                                    egui::StrokeKind::Inside,
                                );
                            }

                            if let Some(payload) = drop_zone.dnd_release_payload::<std::sync::Arc<(String, String)>>() {
                                let (source_path, source_value) = &**payload;
                                if ui.input(|i| i.modifiers.shift) {
                                    *value = format!("={}", source_path); // On lie
                                } else {
                                    *value = source_value.clone();        // On copie purement
                                }
                                cell_changed = true;
                            }

                            if cell_changed {
                                ui_changed = true;
                            }
                        }); // Fin de la colonne (strip.col)
                    } // Fin de la boucle des steps
                }); // Fin de la ligne (body.row)
            } // Fin de la boucle des row_definitions
        }); // Fin du .body

    // 3. LA SUPPRESSION : Tout en bas
    if let Some(idx) = step_to_remove {
        flavor.steps.remove(idx);
        ui_changed = true;
    }

    if let Some((from_idx, to_idx)) = step_to_move {
        if from_idx != to_idx {
            // On retire la colonne de son ancienne place
            let step = flavor.steps.remove(from_idx);
            
            // On la réinsère à sa nouvelle place (en s'assurant de ne pas dépasser la taille)
            flavor.steps.insert(to_idx.min(flavor.steps.len()), step);
            
            // On lève le drapeau pour que Rhai recalcule tout, car les variables absolues
            // ont changé de nom (Step 1 est devenu Step 2, etc.) !
            ui_changed = true; 
        }
    }
    
    ui_changed
}

// ============================================================================
// WIDGET ROUTER
// ============================================================================

fn draw_cell_value(
    ui: &mut egui::Ui,
    value: &mut String,
    widget: &WidgetDef,
    row_key: &str,
    _engine: &ScriptEngine,
    file_dialog: &mut egui_file_dialog::FileDialog,
    pending_target: &mut Option<FileDialogTarget>,
) -> bool {
    let mut value_has_changed = false;

    let mut handle_text_edit = |ui: &mut egui::Ui, val: &mut String| {
        let response = ui.text_edit_singleline(val);
        
        #[cfg(target_os = "android")]
        {
            if response.interact(egui::Sense::click()).clicked() {
                crate::request_android_text_input(val, row_key);
            }
        }
        response
    };

    // --- Validation Engine ---
    let mut is_valid = true;

    if let Some(rule_str) = widget.validation_rule() {
        if !value.is_empty() {
            if let Ok(regex) = regex::Regex::new(rule_str) {
                is_valid = regex.is_match(value);
            }
        }
    }

    // --- Styling Override ---
    ui.scope(|ui| {
        if !is_valid {
            let error_red = egui::Color32::from_rgb(200, 50, 50);
            ui.visuals_mut().override_text_color = Some(error_red);
            ui.visuals_mut().selection.stroke.color = error_red;
            ui.visuals_mut().widgets.inactive.bg_stroke.color = error_red;
            ui.visuals_mut().widgets.hovered.bg_stroke.color = error_red;
        }

        let response = ui.horizontal(|ui| {
            if !is_valid {
                ui.label("⚠️");
            }

            match widget.widget_type {
                SolageWidget::Text => { 
                    if handle_text_edit(ui, value).changed() {
                        value_has_changed = true;
                    }
                },
                SolageWidget::Number => {
                    let mut num = value.parse::<f32>().unwrap_or(0.0);
                    
                    let base_speed = widget.speed.unwrap_or(1.0);
                    
                    let final_speed = if ui.input(|i| i.modifiers.shift) {
                        widget.speed_shift.map(|s| s * 10.0).unwrap_or(base_speed)
                    } else {
                        base_speed
                    };

                    let mut drag = egui::DragValue::new(&mut num).speed(final_speed);
                    
                    if let Some(prec) = widget.precision {
                        drag = drag.max_decimals(prec).min_decimals(prec);
                    }

                    if let Some(min) = widget.min {
                        if let Some(max) = widget.max {
                            drag = drag.clamp_range(min..=max);
                        } else {
                            drag = drag.clamp_range(min..=f32::INFINITY);
                        }
                    } else if let Some(max) = widget.max {
                        drag = drag.clamp_range(f32::NEG_INFINITY..=max);
                    }

                    let drag_response = ui.add(drag);

                    if drag_response.changed() {
                        if let Some(prec) = widget.precision {
                            *value = format!("{:.*}", prec, num);
                        } else {
                            *value = num.to_string();
                        }
                        value_has_changed = true;
                    }

                    #[cfg(target_os = "android")]
                    if drag_response.clicked() {
                        crate::request_android_text_input(value, row_key);
                    }
                },
                SolageWidget::Slider => {
                    let min = widget.min.unwrap_or(0.0);
                    let max = widget.max.unwrap_or(100.0);
                    let mut num = value.parse::<f32>().unwrap_or(min).clamp(min, max);

                    if ui.add(egui::Slider::new(&mut num, min..=max)).changed() {
                        *value = num.to_string();
                        value_has_changed = true;
                    }
                },
                SolageWidget::Bool | SolageWidget::Checkbox => {
                    let mut is_checked = value.parse::<bool>().unwrap_or(false);
                    
                    if ui.checkbox(&mut is_checked, "").changed() {
                        *value = is_checked.to_string();
                        value_has_changed = true; 
                    }
                },
                SolageWidget::Path => {
                    handle_text_edit(ui, value);
                    
                    #[cfg(not(target_arch = "wasm32"))]
                    if ui.button("Browse...").clicked() { 
                        #[cfg(target_os = "android")]
                        crate::request_android_file_picker(row_key);
                        
                        #[cfg(not(target_os = "android"))]
                        {
                            *pending_target = Some(FileDialogTarget::WidgetPath(row_key.to_string()));
                            file_dialog.pick_file(); 
                        }
                    }
                },
                SolageWidget::Dropdown => {
                    let options = widget.options.as_ref().map_or(vec![], |o| o.clone());
                    let current_display = if value.is_empty() { "Select..." } else { value.as_str() };

                    egui::ComboBox::from_id_source(row_key)
                        .selected_text(current_display)
                        .show_ui(ui, |ui| {
                            for opt in options {
                                if ui.selectable_value(value, opt.clone(), &opt).clicked() {
                                    value_has_changed = true;
                                }
                            }
                        });
                }
            }
        }).response;

        if !is_valid {
            response.on_hover_text(format!("Validation Error.\nMust respect rule: {}", widget.validation_rule().unwrap_or("")));
        }
    });

    value_has_changed
}

// ============================================================================
// SYSTEM UTILITIES
// ============================================================================

fn update_widget_value(state: &mut AppState, target_key: &str, new_value: String) {
    state.user_values.insert(target_key.to_string(), new_value);
}

fn apply_defaults(config: &AppConfig, state: &mut AppState) {
    state.nav.section = 0;
    state.nav.mode = 0;
    state.nav.flavor = 0;
}

pub fn apply_studio_theme(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    
    fonts.font_data.insert(
        "StudioFont".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/font.ttf"))), 
    );

    fonts.families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "StudioFont".to_owned());
    ctx.set_fonts(fonts);

    let mut visuals = Visuals::dark();
    
    visuals.window_fill = Color32::from_rgb(25, 27, 31);
    visuals.panel_fill = Color32::from_rgb(18, 20, 24);
    
    let solage_blue = Color32::from_rgb(100, 180, 255);
    let dark_text = Color32::from_rgb(20, 22, 25);

    visuals.selection.bg_fill = solage_blue;
    visuals.selection.stroke = egui::Stroke::new(1.0, dark_text); 
    
    visuals.widgets.active.bg_fill = solage_blue;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, Color32::WHITE);
    
    let radius = egui::CornerRadius::same(6);
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.active.corner_radius = radius;
    visuals.window_corner_radius = egui::CornerRadius::same(10);
    
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    
    style.text_styles.insert(TextStyle::Body, egui::FontId::proportional(15.0));
    style.text_styles.insert(TextStyle::Button, egui::FontId::proportional(15.0));
    style.text_styles.insert(TextStyle::Monospace, egui::FontId::monospace(14.0));
    style.text_styles.insert(TextStyle::Heading, egui::FontId::proportional(26.0));
    
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    
    ctx.set_style(style);
}

#[cfg(target_os = "android")]
extern "C" {
    pub fn ANativeActivity_showSoftInput(activity: *mut std::ffi::c_void, flags: u32);
    pub fn ANativeActivity_hideSoftInput(activity: *mut std::ffi::c_void, flags: u32);
}

#[cfg(target_os = "android")]
pub fn force_android_keyboard(show: bool) {
    let ctx = ndk_context::android_context();
    let activity_ptr = ctx.context();

    if activity_ptr.is_null() { return; }

    unsafe {
        if show {
            ANativeActivity_showSoftInput(activity_ptr, 2); // 2 = SHOW_FORCED
        } else {
            ANativeActivity_hideSoftInput(activity_ptr, 0); // 0 = HIDE_IMPLICIT_ONLY
        }
    }
}

#[cfg(not(target_os = "android"))]
pub fn force_android_keyboard(_show: bool) {}
