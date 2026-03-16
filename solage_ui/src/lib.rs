use egui::{Ui, Color32, RichText, Visuals, TextStyle};
use egui_extras::{TableBuilder, Column};
use egui_file_dialog::FileDialog;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use rhai::{Engine, Scope, Map, Dynamic};

use solage_data::{AppConfig, NavState, WidgetDef, Flavor, AppState, GlobalPreferences, WidgetType as SolageWidget}; 
use solage_core::{ScriptEngine, PlatformBackend, ScriptContext, AuthProvider, AuthState, load_config, load_state, save_state, save_preferences, load_preferences};
use std::sync::Mutex;

// La file d'attente pour récupérer les textes
pub static PENDING_TEXT_UPDATES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

// Le callback appelé par Android quand on clique sur "OK"
#[cfg(target_os = "android")]
#[no_mangle]
// ON REVIENT À "C" POUR QUE LA JVM TROUVE LA FONCTION !
pub extern "C" fn Java_com_cloudcompositing_solage_MainActivity_onTexteSaisi(
    mut env: jni::JNIEnv,
    _this: jni::objects::JObject, // On garde bien JObject (l'instance) et non JClass !
    j_row_key: jni::objects::JString,
    j_texte: jni::objects::JString,
) {
    let row_key: String = match env.get_string(&j_row_key) {
        Ok(java_str) => java_str.into(),
        Err(_) => String::new(),
    };

    let texte: String = match env.get_string(&j_texte) {
        Ok(java_str) => java_str.into(),
        Err(_) => String::new(),
    };

    if let Ok(mut queue) = PENDING_TEXT_UPDATES.lock() {
        queue.push((row_key, texte));
    }
}

// L'appel pour ouvrir la pop-up depuis Rust
#[cfg(target_os = "android")]
pub fn demander_texte_android(texte_actuel: &str, row_key: &str) {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm();
    let context_ptr = ctx.context();

    if vm_ptr.is_null() || context_ptr.is_null() { return; }

    unsafe {
        if let Ok(jvm) = jni::JavaVM::from_raw(vm_ptr.cast()) {
            
            // LA CORRECTION CRITIQUE : On récupère l'environnement sans déclencher 
            // le détachement automatique à la fin de la fonction !
            let mut env = match jvm.get_env() {
                Ok(e) => e,
                Err(_) => jvm.attach_current_thread_permanently().unwrap(),
            };

            let activity = jni::objects::JObject::from_raw(context_ptr.cast());
            
            // On s'assure que la création des chaînes ne panique pas
            if let (Ok(j_texte), Ok(j_key)) = (env.new_string(texte_actuel), env.new_string(row_key)) {
                let _ = env.call_method(
                    &activity,
                    "demanderTexte",
                    "(Ljava/lang/String;Ljava/lang/String;)V",
                    &[(&j_texte).into(), (&j_key).into()]
                );
            }
        }
    }
}

// 1. La nouvelle file d'attente
pub static PENDING_FILE_UPDATES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

// 2. La réception (Appelée par Kotlin)
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_com_cloudcompositing_solage_MainActivity_onFichierChoisi(
    mut env: jni::JNIEnv,
    _this: jni::objects::JObject,
    j_row_key: jni::objects::JString,
    j_uri: jni::objects::JString,
) {
    let row_key: String = env.get_string(&j_row_key).map(|s| s.into()).unwrap_or_default();
    let uri: String = env.get_string(&j_uri).map(|s| s.into()).unwrap_or_default();

    if let Ok(mut queue) = crate::PENDING_FILE_UPDATES.lock() {
        queue.push((row_key, uri));
    }
}

// 3. L'envoi (Appelée par egui)
#[cfg(target_os = "android")]
pub fn choisir_fichier_android(row_key: &str) {
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
                    "choisirFichier",
                    "(Ljava/lang/String;)V",
                    &[(&j_key).into()]
                );
            }
        }
    }
}

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
    WidgetPath(String), // Contient la clé (key) du widget pour le mettre à jour
}

pub struct SolageApp {
    pub backend: Box<dyn PlatformBackend>,
    pub state: AppState,
    pub engine: ScriptEngine,
    pub preferences: GlobalPreferences,
    config: Option<AppConfig>, 
    current_config_path: Option<PathBuf>,
    error_msg: Option<String>,
    prefs_path: String,
    login_form: LoginForm,
    auth: Box<dyn AuthProvider>,
    pub url_input: String,
    pub download_rx: Option<Receiver<Result<String, String>>>,
    pub file_load_rx: Option<Receiver<PathBuf>>,
    pub toast: Option<(String, f64)>,
    pub theme_applied: bool,
    pub file_dialog: FileDialog,
    pub pending_file_target: Option<FileDialogTarget>,
    pub external_file_rx: Option<std::sync::mpsc::Receiver<(String, String)>>,
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

        let app = Self {
            backend,
            auth,
            url_input: default_url,
            state: AppState::default(),
            engine: ScriptEngine::new(),
            preferences,
            login_form: LoginForm::default(),
            config: None,
            current_config_path: None,
            error_msg: None,
            prefs_path: prefs_path.to_string_lossy().to_string(),
            download_rx: None,
            file_load_rx: None,
            toast: None,
            theme_applied: false,
            file_dialog: FileDialog::new(),
            pending_file_target: None,
            external_file_rx: None,
        };

        app
    }

    pub fn load_yaml_string(&mut self, yaml_content: &str) {
        match load_config(yaml_content) {
            Ok(config) => apply_defaults(&config, &mut self.state),
            Err(e) => self.error_msg = Some(format!("Erreur YAML: {}", e)),
        }
    }

    fn load_config_from_path(&mut self, path: &Path) {
        log::info!("Lecture du fichier : {:?}", path);

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                self.error_msg = Some(format!("Impossible de lire le fichier :\n{}", e));
                return;
            }
        };

        match load_config(&content) {
            Ok(cfg) => {
                log::info!("YAML parsé avec succès. Sections trouvées : {}", cfg.sections.len());
                self.error_msg = None; 

                self.config = Some(cfg.clone());
                self.current_config_path = Some(path.to_path_buf());
                
                let state_path = path.with_extension("json");
                if let Ok(loaded_state) = load_state(&state_path.to_string_lossy()) {
                     self.state = loaded_state;
                     if self.state.config.sections.is_empty() {
                         self.state.config = cfg.clone();
                     }
                } else {
                    apply_defaults(&cfg, &mut self.state);
                }
                
                if self.state.config.sections.is_empty() {
                    self.error_msg = Some("Le fichier est valide mais ne contient aucune section.".to_string());
                }

                self.add_to_recents(path.to_path_buf());
            },
            Err(e) => {
                log::error!("Erreur Parsing YAML : {}", e);
                self.error_msg = Some(format!("Erreur de syntaxe YAML :\n{}", e));
            }
        }
    }

    fn add_to_recents(&mut self, path: PathBuf) {
        self.preferences.recent_files.retain(|p| p != &path);
        self.preferences.recent_files.insert(0, path);
        if self.preferences.recent_files.len() > 5 { 
            self.preferences.recent_files.truncate(5); 
        }
        let _ = save_preferences(&self.prefs_path, &self.preferences);
    }

    fn fetch_url(&mut self, url: &str, ctx: &egui::Context) {
        self.url_input = url.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        self.download_rx = Some(rx);
        let ctx_clone = ctx.clone();
        
        let request = ehttp::Request::get(url);
        ehttp::fetch(request, move |response| {
            let result = match response {
                Ok(res) if res.ok => Ok(res.text().unwrap_or("").to_string()),
                Ok(res) => Err(format!("Erreur {}: {}", res.status, res.status_text)),
                Err(e) => Err(format!("Erreur réseau : {}", e)),
            };
            let _ = tx.send(result);
            ctx_clone.request_repaint(); 
        });
    }

    fn draw_login_screen(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {

            ui.vertical_centered(|ui| {
                ui.heading("Connexion");
                ui.add_space(20.0);
                
                ui.horizontal(|ui| {
                    ui.label("Utilisateur:");
                    ui.text_edit_singleline(&mut self.login_form.username);
                });
                ui.horizontal(|ui| {
                    ui.label("Mot de passe:");
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
                
                if ui.button("Se connecter").clicked() {
                    self.auth.login(
                        &self.login_form.username.clone(),
                        &self.login_form.password.clone(),
                        ctx,
                    );
                }
            });
        });

    }

    pub fn mettre_a_jour_valeur_yaml(&mut self, row_key: &str, nouvelle_valeur: String) {
        
        // On récupère les indices actuels depuis la navigation
        let section_idx = self.state.nav.section;
        let mode_idx = self.state.nav.mode;
        let flavor_idx = self.state.nav.flavor;
        let step_idx = self.state.nav.step;

        // Navigation sécurisée dans l'arbre (Si self.state.config est votre racine)
        if let Some(section) = self.state.config.sections.get_mut(section_idx) {
            if let Some(mode) = section.modes.get_mut(mode_idx) {
                if let Some(flavor) = mode.flavors.get_mut(flavor_idx) {
                    
                    // --- OPTION A : Injection dans le Step (HashMap) ---
                    if let Some(step) = flavor.steps.get_mut(step_idx) {
                        step.values.insert(row_key.to_string(), nouvelle_valeur.clone());
                        println!("SOLAGE : Dictionnaire du Step '{}' mis à jour -> {} = {}", 
                            step.name, row_key, nouvelle_valeur);
                    }

                    // --- OPTION B : Injection dans le WidgetDef (État de l'UI) ---
                    // for row_def in &mut flavor.row_definitions {
                    //     if row_def.key == row_key {
                    //         row_def.widget.value = Some(nouvelle_valeur.clone());
                    //         println!("SOLAGE : Widget '{}' mis à jour avec la valeur '{}'", 
                    //             row_def.key, nouvelle_valeur);
                    //         break; // On arrête de chercher une fois trouvé
                    //     }
                    // }
                }
            }
        }
    }

    /// Évalue un script Rhai en lui donnant accès à toutes les valeurs du Step actuel
    pub fn evaluer_compute(
        // &self, 
        compute_script: &str, 
        valeurs_actuelles: &std::collections::HashMap<String, String>
    ) -> Result<String, Box<rhai::EvalAltResult>> {
        
        // 1. Instanciation du moteur (Idéalement, gardez cet Engine en cache dans SolageApp 
        // pour ne pas le recréer à chaque fois, mais on le fait ici pour l'exemple)
        let engine = Engine::new();
        let mut scope = Scope::new();

        // 2. Conversion du HashMap Rust en Map Rhai
        let mut rhai_map = Map::new();
        for (cle, valeur) in valeurs_actuelles.iter() {
            // Puisque vos données sont des String, on les passe telles quelles.
            // (Dans le script Rhai, il faudra utiliser parse_float() pour faire des maths)
            rhai_map.insert(cle.clone().into(), Dynamic::from(valeur.clone()));
        }
        
        // On injecte le dictionnaire sous le nom "values" dans le script
        scope.push("values", rhai_map);

        // 3. Évaluation du script
        // On s'attend à ce que le script retourne une valeur (Dynamic)
        let resultat: Dynamic = engine.eval_with_scope(&mut scope, compute_script)?;

        // 4. On convertit le résultat dynamique de Rhai en String pour votre HashMap
        Ok(resultat.to_string())
    }
}

impl eframe::App for SolageApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {


        #[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
        {
            self.file_dialog.update(ctx);
            
            if let Some(path) = self.file_dialog.take_picked() {
                if let Some(target) = self.pending_file_target.clone() {
                    match target {
                        FileDialogTarget::MainConfig => {
                            self.load_config_from_path(&path);
                        }
                        FileDialogTarget::WidgetPath(key) => {
                            // On met à jour la valeur dans l'état (AppState)
                            let path_str = path.display().to_string();
                            // Une petite fonction pour chercher la clé dans la configuration actuelle
                            update_widget_value(&mut self.state, &key, path_str);
                        }
                    }
                }
                self.pending_file_target = None;
            }
        }
        
        #[cfg(any(target_arch = "wasm32", target_os = "android"))]
        if let Some(rx) = &self.external_file_rx {
            if let Ok((file_name, content)) = rx.try_recv() {
                self.load_yaml_string(&content);
                self.current_config_path = Some(PathBuf::from(&file_name));
                if let Ok(saved_state) = load_state(&file_name) {
                    self.state = saved_state;
                }
                self.external_file_rx = None;
            }
        }
        
        let is_mobile = ctx.content_rect().width() < 600.0;

        log::info!("Frame update, is_mobile={}", is_mobile);

        // 1. Le "Bumper" du haut (Encoche / Caméra)    
        #[cfg(target_os = "android")]
        egui::TopBottomPanel::top("android_safe_area_top")
            .frame(egui::Frame::none()) // Totalement invisible
            .exact_height(45.0)         // On repasse en f32 ! Ajustez selon l'encoche
            .show(ctx, |_ui| {});

        // 2. Le "Bumper" du bas (Barre de navigation / Coins)
        #[cfg(target_os = "android")]
        egui::TopBottomPanel::bottom("android_safe_area_bottom")
            .frame(egui::Frame::none()) // Totalement invisible
            .exact_height(35.0)         // Espace pour le balayage
            .show(ctx, |_ui| {});

        if !self.theme_applied {
            apply_studio_theme(ctx);
            self.theme_applied = true;
        }

        if let Some(rx) = &self.download_rx {
            if let Ok(result) = rx.try_recv() {
                self.download_rx = None; 
                match result {
                    Ok(yaml) => {
                        self.load_yaml_string(&yaml);
                        
                        if self.error_msg.is_none() {
                            let url_path = PathBuf::from(&self.url_input);
                            self.add_to_recents(url_path.clone());
                            
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

        if let Some(rx) = &self.file_load_rx {
            if let Ok(path) = rx.try_recv() {
                self.load_config_from_path(&path);
                self.file_load_rx = None; // On vide le canal
            }
        }

        self.auth.poll();

        if let AuthState::LoggedIn { .. } = self.auth.state() {
            if self.toast.is_none() {
            }
        }

        log::info!("auth is_ready: {}", self.auth.is_ready());
        if !self.auth.is_ready() {
            self.draw_login_screen(ctx);
            return;
        }

        #[cfg(not(target_os = "android"))] 
        {
            let time = ctx.input(|i| i.time);
            if time < 2.5 {
                if time < 1.5 {
                    draw_splash_anim(ctx, time);
                    ctx.request_repaint();
                    return; 
                }
            }
        }

        if self.state.config.sections.is_empty() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("🛠️ BUILD: Test API Insets 8")
                        .color(egui::Color32::RED)
                        .size(24.0)
                        .strong()
                );
                if let Some(err) = &self.error_msg {
                    ui.group(|ui| {
                        ui.colored_label(Color32::RED, "🛑 Erreur");
                        ui.monospace(err); 
                    });
                    ui.add_space(20.0);
                }

                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.heading(RichText::new("SOLAGE").size(60.0).strong().color(Color32::from_rgb(100, 180, 255)));
                    ui.label(RichText::new("Pipeline Manager").size(20.0).color(Color32::GRAY));
                });
                ui.add_space(40.0);

                ui.vertical_centered(|ui| {
                    // On retire le `if !is_mobile` pour que le bouton s'affiche sur Android !
                    let btn = egui::Button::new(egui::RichText::new("📂 Ouvrir fichier local...").size(16.0))
                        .min_size(egui::vec2(300.0, 35.0));

                    if ui.add(btn).clicked() {
                        
                        // --- BIFURCATION DESKTOP (Windows/Linux/Mac) ---
                        #[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
                        {
                            self.pending_file_target = Some(FileDialogTarget::MainConfig);
                            self.file_dialog.pick_file(); 
                        }
                        
                        // --- BIFURCATION WEB (WASM) ---
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

                        // --- BIFURCATION MOBILE (Android) ---
                        #[cfg(target_os = "android")]
                        {
                            let (tx, rx) = std::sync::mpsc::channel();
                            self.external_file_rx = Some(rx);
                            // On lance votre méthode asynchrone pour le sélecteur natif
                            self.backend.pick_file_async_mobile(tx);
                        }
                    }
                    ui.add_space(15.0);

                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                        ui.horizontal(|ui| {
                            // 1. On vide la file d'attente (mise à jour asynchrone)
                            #[cfg(target_os = "android")]
                            if let Ok(mut queue) = PENDING_TEXT_UPDATES.lock() {
                                for (cle, nouveau_texte) in queue.drain(..) {
                                    if cle == "url_input" {
                                        self.url_input = nouveau_texte;
                                    }
                                }
                            }

                            // Dans votre impl eframe::App pour SolageApp, méthode update() :
                            #[cfg(target_os = "android")]
                            if let Ok(mut queue) = PENDING_FILE_UPDATES.lock() {
                                for (row_key, uri) in queue.drain(..) {
                                    // On réutilise votre fonction magique !
                                    self.mettre_a_jour_valeur_yaml(&row_key, uri);
                                }
                            }

                            // 2. Le TextEdit
                            let response = ui.add(egui::TextEdit::singleline(&mut self.url_input)
                                .desired_width(220.0));

                            // 3. L'appel de la pop-up au clic
                            #[cfg(target_os = "android")]
                            if response.clicked() {
                                // Pas d'erreur de borrow checker ici car on utilise "self.url_input" sans &mut
                                demander_texte_android(&self.url_input, "url_input");
                            }

                            // 4. Le bouton
                            if ui.add(egui::Button::new("⬇ URL")
                                .min_size(egui::vec2(70.0, 30.0))).clicked() {
                                let url = self.url_input.clone();
                                self.fetch_url(&url, ctx);
                            }
                        });
                    });

                    if !self.preferences.recent_files.is_empty() {
                        ui.add_space(30.0);
                        ui.separator();
                        ui.add_space(10.0);
                        ui.label(RichText::new("RÉCEMMENT OUVERTS").size(12.0).strong().color(Color32::GRAY));
                        ui.add_space(10.0);

                        let recent_files = self.preferences.recent_files.clone();
                        for path in recent_files {
                            let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                            let full_path = path.to_string_lossy();
                            
                            ui.scope(|ui| {
                                ui.style_mut().visuals.widgets.inactive.weak_bg_fill = Color32::from_gray(30);
                                let btn = egui::Button::new(
                                    RichText::new(format!("📄 {}", file_name)).size(14.0))
                                    .min_size(egui::vec2(280.0, 28.0))
                                    .frame(true);

                                if ui.add(btn).on_hover_text(full_path.to_string()).clicked() {
                                    let path_str = full_path.to_string();
                                    if path_str.starts_with("http") {
                                        self.fetch_url(&path_str, ctx);
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

            #[cfg(not(target_os = "android"))]
            {
                let time = ctx.input(|i| i.time);
                if time >= 1.5 && time < 2.5 {
                    draw_splash_anim(ctx, time);
                    ctx.request_repaint();
                }
            }
            return;
        }

        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            if let Some(path) = &self.current_config_path {
                let path_str = path.to_string_lossy().to_string();
                let state_path = if path_str.starts_with("http") {
                    path_str
                } else {
                    path.with_extension("json").to_string_lossy().to_string()
                };
                let _ = save_state(&state_path, &self.state);
                self.toast = Some(("✅ Projet sauvegardé".to_string(), ctx.input(|i| i.time)));
                log::info!("Sauvegarde effectuée via raccourci !");
            }
        }
        
        let mut global_map = HashMap::new();
        for section in &self.state.config.sections {
            for mode in &section.modes {
                for flavor in &mode.flavors {
                    for step in &flavor.steps {
                        for (key, val) in &step.values {
                            global_map.insert(key.clone(), val.clone());
                        }
                    }
                }
            }
        }
        let mut script_context = self.engine.build_context(&global_map);

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Solage");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    
                    if ui.button("Fermer Projet").clicked() {
                        if let Some(path) = &self.current_config_path {
                            let path_str = path.to_string_lossy().to_string();
                            let state_path = if path_str.starts_with("http") {
                                path_str
                            } else {
                                path.with_extension("json").to_string_lossy().to_string()
                            };
                            let _ = save_state(&state_path, &self.state);
                        }
                        
                        self.state.config.sections.clear();
                        self.current_config_path = None;
                    }

                    if ui.button("💾 Save").clicked() {
                        if let Some(path) = &self.current_config_path {
                            let path_str = path.to_string_lossy().to_string();
                            let state_path = if path_str.starts_with("http") {
                                path_str
                            } else {
                                path.with_extension("json").to_string_lossy().to_string()
                            };
                            let _ = save_state(&state_path, &self.state);
                            self.toast = Some(("✅ Projet sauvegardé".to_string(), ctx.input(|i| i.time)));
                        }
                    }

                    ui.label(format!("{} sections", self.state.config.sections.len()));
                });
            });
        });

        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Actions :");
                for action in &self.state.config.actions {
                    if ui.button(&action.label).clicked() {
                        self.engine.run_action(&action.script, &global_map);
                    }
                }
            });
        });

        if is_mobile {
            egui::TopBottomPanel::bottom("mobile_nav").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let available = ui.available_width();
                    let btn_width = available / self.state.config.sections.len() as f32;
                    for (idx, section) in self.state.config.sections.iter().enumerate() {
                        let is_selected = self.state.nav.section == idx;
                        let label = format!("{}\n{}", section.icon, section.name);
                        if ui.add_sized(
                            [btn_width, 50.0],
                            egui::Button::selectable(is_selected, RichText::new(&label).size(11.0))
                        ).clicked() {
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
                for (idx, section) in self.state.config.sections.iter().enumerate() {
                    let is_selected = self.state.nav.section == idx;
                    let label = format!("{} {}", section.icon, section.name);
                    if ui.selectable_label(is_selected, label).clicked() {
                        self.state.nav.section = idx;
                        self.state.nav.mode = 0;
                        self.state.nav.flavor = 0;
                    }
                }
                ui.add_space(20.0);
                if ui.button("📂 Ouvrir autre...").clicked() {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        self.pending_file_target = Some(FileDialogTarget::MainConfig);
                        self.file_dialog.pick_file(); 
                    }
                    
                    // Sur le Web, on lance la boite d'upload du navigateur !
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

        #[cfg(target_os = "android")]
        if let Ok(mut queue) = PENDING_TEXT_UPDATES.lock() {
            // S'il y a des données, on les dépile
            for (row_key, nouveau_texte) in queue.drain(..) {
                
                // Routage selon la clé
                if row_key == "url_input" {
                    self.url_input = nouveau_texte;
                } else {
                    // C'est ici que la magie opère pour vos cellules !
                    // On envoie la clé et la valeur à votre gestionnaire YAML
                    self.mettre_a_jour_valeur_yaml(&row_key, nouveau_texte);
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(err) = &self.error_msg {
                ui.colored_label(Color32::RED, format!("⚠ {}", err));
            }

            let s_idx = self.state.nav.section;
            if let Some(active_section) = self.state.config.sections.get_mut(s_idx) {
                ui.horizontal(|ui| {
                    for (m_idx, mode) in active_section.modes.iter().enumerate() {
                        let is_sel = self.state.nav.mode == m_idx;
                        if ui.selectable_label(is_sel, &mode.name).clicked() {
                            self.state.nav.mode = m_idx;
                            self.state.nav.flavor = 0;
                        }
                    }
                });
                ui.separator();

                let m_idx = self.state.nav.mode;
                if let Some(active_mode) = active_section.modes.get_mut(m_idx) {
                    ui.horizontal(|ui| {
                        ui.label("Variante :");
                        let f_idx = self.state.nav.flavor;
                        let current_name = active_mode.flavors.get(f_idx).map(|f| f.name.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("flav_cb")
                            .selected_text(current_name)
                            .show_ui(ui, |ui| {
                                for (i, f) in active_mode.flavors.iter().enumerate() {
                                    ui.selectable_value(&mut self.state.nav.flavor, i, &f.name);
                                }
                            });
                    });
                    
                    let f_idx = self.state.nav.flavor;
                    if let Some(active_flavor) = active_mode.flavors.get_mut(f_idx) {
                        if is_mobile {
                            draw_single_step(ui, active_flavor, &mut script_context, &self.engine, &mut self.file_dialog, &mut self.pending_file_target, &mut self.state.nav);
                        } else {
                            draw_comparison_table(ui, active_flavor, &mut script_context, &self.engine, &mut self.file_dialog, &mut self.pending_file_target);
                        }
                    }
                }
            }
        });
        
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
    _script_context: &mut ScriptContext,
    engine: &ScriptEngine,
    file_dialog: &mut egui_file_dialog::FileDialog, // Remplace backend
    pending_target: &mut Option<FileDialogTarget>,
    nav: &mut NavState,
) {
    if flavor.steps.is_empty() {
        ui.colored_label(Color32::GRAY, "Aucun step défini");
        return;
    }

    let step_count = flavor.steps.len();
    let current = nav.step.min(step_count - 1);

    ui.horizontal(|ui| {
        if ui.button("◀").clicked() && current > 0 {
            nav.step -= 1;
        }
        ui.strong(RichText::new(&flavor.steps[current].name)
            .size(16.0)
            .color(Color32::from_rgb(100, 180, 255)));
        if ui.button("▶").clicked() && current < step_count - 1 {
            nav.step += 1;
        }
        ui.label(format!("{}/{}", current + 1, step_count));
    });
    ui.separator();

    let step = &mut flavor.steps[current];
    for row_def in &flavor.row_definitions {
        ui.horizontal(|ui| {
            ui.set_min_width(ui.available_width());
            ui.label(&row_def.label);
            let value = step.values
                .entry(row_def.key.clone())
                .or_insert_with(|| {
                    row_def.widget.default.as_ref()
                        .map(|d| match d {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            _ => String::new(),
                        })
                        .unwrap_or_default()
                });
            let a_change = draw_cell_value(ui, value, &row_def.widget, &row_def.key, engine, file_dialog, pending_target);

            // Si l'utilisateur vient de modifier CETTE cellule, on déclenche le Compute
            if a_change {
                for autre_row in &flavor.row_definitions {
                    if let Some(script_rhai) = autre_row.widget.compute_rule() {
                        
                        // On appelle notre fonction statique
                        if let Ok(nouveau_resultat) = SolageApp::evaluer_compute(script_rhai, &step.values) {
                            // On met à jour la valeur calculée silencieusement !
                            println!("Calcul Rhai réussi pour {} : {}", autre_row.key, nouveau_resultat);
                            step.values.insert(autre_row.key.clone(), nouveau_resultat);
                        }
                    }
                }
            }
        });
    }
}

fn draw_comparison_table(
    ui: &mut Ui, 
    flavor: &mut Flavor, 
    _script_context: &mut ScriptContext, 
    engine: &ScriptEngine, 
    file_dialog: &mut egui_file_dialog::FileDialog,
    pending_target: &mut Option<FileDialogTarget>,
) {
    if flavor.steps.is_empty() {
        ui.colored_label(Color32::GRAY, "Aucun step défini");
        return;
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
                ui.strong(RichText::new("Paramètre").color(Color32::WHITE)); 
            });
            for step in &flavor.steps {
                header.col(|ui| { 
                    ui.strong(RichText::new(&step.name).color(Color32::from_rgb(100, 180, 255))); 
                });
            }
        })
        .body(|mut body| {
            for row_def in &flavor.row_definitions {
                body.row(24.0, |mut strip| {
                    // Colonne label
                    strip.col(|ui| { ui.label(&row_def.label); });

                    // Une colonne par step
                    for step in &mut flavor.steps {
                        strip.col(|ui| {
                            let value = step.values
                                .entry(row_def.key.clone())
                                .or_insert_with(|| {
                                    row_def.widget.default
                                        .as_ref()
                                        .map(|d| match d {
                                            serde_json::Value::String(s) => s.clone(),
                                            serde_json::Value::Number(n) => n.to_string(),
                                            serde_json::Value::Bool(b) => b.to_string(),
                                            _ => String::new(),
                                        })
                                        .unwrap_or_default()
                                });

                            let a_change = draw_cell_value(ui, value, &row_def.widget, &row_def.key, engine, file_dialog, pending_target);

                            // Si l'utilisateur vient de modifier CETTE cellule, on déclenche le Compute
                            if a_change {
                                for autre_row in &flavor.row_definitions {
                                    if let Some(script_rhai) = autre_row.widget.compute_rule() {
                                        
                                        // On appelle notre fonction statique
                                        if let Ok(nouveau_resultat) = SolageApp::evaluer_compute(script_rhai, &step.values) {
                                            // On met à jour la valeur calculée silencieusement !
                                            println!("Calcul Rhai réussi pour {} : {}", autre_row.key, nouveau_resultat);
                                            step.values.insert(autre_row.key.clone(), nouveau_resultat);
                                        }
                                    }
                                }
                            }
                        });
                    }
                });
            }
        });
}

fn draw_cell_value(
    ui: &mut egui::Ui,
    value: &mut String,
    widget: &WidgetDef,
    row_key: &str,
    _engine: &ScriptEngine,
    file_dialog: &mut egui_file_dialog::FileDialog,
    pending_target: &mut Option<FileDialogTarget>,
) -> bool {
    log::info!("draw_cell_value appelé, widget={:?}", widget.widget_type);
    
    let mut valeur_a_change = false;

    let mut handle_text_edit = |ui: &mut egui::Ui, val: &mut String| {
        let response = ui.text_edit_singleline(val);
        
        #[cfg(target_os = "android")]
        {
            // Votre correctif : on force la détection du clic même si le TextEdit l'a consommé
            if response.interact(egui::Sense::click()).clicked() {
                // On appelle notre boîte de dialogue native robuste
                demander_texte_android(val, row_key);
            }
        }
        
        response
    };

    // ---------------------------------------------------------
    // MOTEUR DE VALIDATION
    // ---------------------------------------------------------
    let mut est_valide = true;

    // On vérifie si ce widget possède une règle de validation
    if let Some(regle_str) = widget.validation_rule() {
        // Si la valeur n'est pas vide, on la teste contre la Regex
        if !value.is_empty() {
            // Note: Pour des performances optimales à 60 FPS, il faudrait mettre en cache 
            // les regex compilées. Mais pour l'UI, cette compilation à la volée est acceptable.
            if let Ok(regex) = regex::Regex::new(regle_str) {
                est_valide = regex.is_match(value);
            }
        }
    }

    // ---------------------------------------------------------
    // MODIFICATION DU STYLE (Si invalide)
    // ---------------------------------------------------------
    // egui utilise ui.scope pour isoler les changements de style à ce widget uniquement
    ui.scope(|ui| {
        
        // Si la validation échoue, on teint l'interface en rouge
        if !est_valide {
            let rouge_erreur = egui::Color32::from_rgb(200, 50, 50);
            
            // On force le texte et les bordures en rouge
            ui.visuals_mut().override_text_color = Some(rouge_erreur);
            ui.visuals_mut().selection.stroke.color = rouge_erreur;
            ui.visuals_mut().widgets.inactive.bg_stroke.color = rouge_erreur;
            ui.visuals_mut().widgets.hovered.bg_stroke.color = rouge_erreur;
        }

        // On enveloppe le widget dans une UI qui gérera l'info-bulle
        let response = ui.horizontal(|ui| {
            
            // L'indicateur visuel (Optionnel : un petit icône d'avertissement)
            if !est_valide {
                ui.label("⚠️");
            }

            // =========================================================
            // ICI VIENT VOTRE GRAND MATCH SUR LE TYPE DE WIDGET
            // =========================================================
            match widget.widget_type {
                SolageWidget::Text => { /* ... */ },
                SolageWidget::Number => { /* ... */ },
                // ... etc ...
                _ => {}
            }
            
        }).response;

        // Si c'est invalide et que l'utilisateur survole la zone, on explique pourquoi
        if !est_valide {
            response.on_hover_text(format!("Erreur de validation.\nDoit respecter la règle : {}", widget.validation_rule().unwrap_or("")));
        }
    });

    // 2. On utilise notre nouvelle fonction dans le match
    match widget.widget_type {
        SolageWidget::Text => { 
            if handle_text_edit(ui, value).changed() {
                valeur_a_change = true;
            }
        },
        SolageWidget::Number => {
            let mut num = value.parse::<f32>().unwrap_or(0.0);
            
            // 1. LE DIAGNOSTIC : On imprime ce que Rust a réellement compris du YAML
            log::info!("SOLAGE DEBUG - Widget '{}' -> Speed lue : {:?}, Precision lue : {:?}", 
                row_key, widget.speed, widget.precision);

            // 2. On lit la vitesse de base dictée par le YAML
            let mut base_speed = widget.speed.unwrap_or(1.0);
            
            // 3. LE CORRECTIF MOBILE : L'amortisseur de DPI
            // #[cfg(target_os = "android")]
            // {
            //     // On divise drastiquement la vitesse sur mobile pour compenser 
            //     // la sensibilité extrême du tactile sur les écrans haute densité.
            //     base_speed *= 0.05; 
            // }

            // 4. On gère le Shift (Pour le clavier PC)
            let vitesse_finale = if ui.input(|i| i.modifiers.shift) {
                widget.speed_shift.map(|s| s * 10.0).unwrap_or(base_speed)
            } else {
                base_speed
            };

            let mut drag = egui::DragValue::new(&mut num).speed(vitesse_finale);
            
            // ... (Le reste de votre code avec precision et min/max reste identique) ...  
            // --- NOUVEAU : APPLICATION DE LA PRÉCISION VISUELLE ---
            if let Some(prec) = widget.precision {
                // Force egui à ne pas afficher plus de décimales que prévu
                drag = drag.max_decimals(prec)
                           .min_decimals(prec); // Gardez min_decimals si vous voulez forcer l'affichage des zéros (ex: 1.700)
            }

            // Application des limites
            if let Some(min) = widget.min {
                if let Some(max) = widget.max {
                    drag = drag.clamp_range(min..=max);
                } else {
                    drag = drag.clamp_range(min..=f32::INFINITY);
                }
            } else if let Some(max) = widget.max {
                drag = drag.clamp_range(f32::NEG_INFINITY..=max);
            }

            let response = ui.add(drag);

            // --- NOUVEAU : FORMATAGE PROPRE POUR LA SAUVEGARDE ---
            if response.changed() {
                if let Some(prec) = widget.precision {
                    // La macro {:.*} prend la précision (prec) puis le nombre (num)
                    // Si prec = 0, num = 1920.5 -> "*value" deviendra "1920"
                    // Si prec = 2, num = 1.7 -> "*value" deviendra "1.70"
                    *value = format!("{:.*}", prec, num);
                } else {
                    *value = num.to_string();
                }
                valeur_a_change = true;
            }

            #[cfg(target_os = "android")]
            if response.clicked() {
                crate::demander_texte_android(value, row_key);
            }
        },
        SolageWidget::Slider => {
            // Un slider a besoin de limites strictes. On définit des valeurs par défaut si absentes.
            let min = widget.min.unwrap_or(0.0);
            let max = widget.max.unwrap_or(100.0);
            
            let mut num = value.parse::<f32>().unwrap_or(min);
            
            // On limite la valeur actuelle pour qu'elle ne sorte pas du slider au chargement
            num = num.clamp(min, max);

            if ui.add(egui::Slider::new(&mut num, min..=max)).changed() {
                *value = num.to_string();
                valeur_a_change = true;
            }
        },
        SolageWidget::Bool | SolageWidget::Checkbox => {
            // 1. On déclare bien la variable sous le nom "is_checked"
            let mut is_checked = value.parse::<bool>().unwrap_or(false);
            
            // 2. On la passe à egui
            if ui.checkbox(&mut is_checked, "").changed() {
                *value = is_checked.to_string();
                valeur_a_change = true; // On déclenche le Compute Rhai !
            }
        },
        SolageWidget::Path => {
            ui.horizontal(|ui| {
                handle_text_edit(ui, value);
                
                #[cfg(not(target_arch = "wasm32"))]
                if ui.button("Ouvrir...").clicked() { 
                    
                    // --- BIFURCATION ANDROID ---
                    #[cfg(target_os = "android")]
                    {
                        // Sur mobile, on déclenche l'Intent Kotlin
                        crate::choisir_fichier_android(row_key);
                    }
                    
                    // --- BIFURCATION DESKTOP ---
                    #[cfg(not(target_os = "android"))]
                    {
                        // Sur PC, on garde votre logique de dialogue interne
                        *pending_target = Some(FileDialogTarget::WidgetPath(row_key.to_string()));
                        file_dialog.pick_file(); 
                    }
                }
            });
        },
        SolageWidget::Dropdown => {
            // On récupère les options de votre configuration YAML
            let options = widget.options.as_ref().map_or(vec![], |o| o.clone());
            
            // On s'assure que la valeur actuelle existe, sinon on prend "Sélectionner..."
            let affichage_actuel = if value.is_empty() {
                "Sélectionner..."
            } else {
                value.as_str()
            };

            // from_id_source évite les conflits si plusieurs dropdowns ont le même contenu
            egui::ComboBox::from_id_source(row_key)
                .selected_text(affichage_actuel)
                .show_ui(ui, |ui| {
                    for opt in options {
                        // selectable_value met à jour 'value' automatiquement si on clique
                        if ui.selectable_value(value, opt.clone(), &opt).clicked() {
                            // Action optionnelle au clic (egui a déjà mis à jour la String)
                        }
                    }
                });
        }
    }

    valeur_a_change
}

fn update_widget_value(state: &mut AppState, target_key: &str, new_value: String) {
    for section in &mut state.config.sections {
        for mode in &mut section.modes {
            for flavor in &mut mode.flavors {
                for step in &mut flavor.steps {
                    // Si on trouve la clé dans les valeurs de l'étape courante, on la met à jour
                    if step.values.contains_key(target_key) {
                        step.values.insert(target_key.to_string(), new_value.clone());
                        return;
                    }
                    // Ou on la crée si elle n'y était pas encore
                    for row in &flavor.row_definitions {
                        if row.key == target_key {
                            step.values.insert(target_key.to_string(), new_value.clone());
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn apply_defaults(config: &AppConfig, state: &mut AppState) {
    state.config = config.clone();
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
pub fn forcer_clavier_android(afficher: bool) {
    let ctx = ndk_context::android_context();
    let activity_ptr = ctx.context();

    if activity_ptr.is_null() { return; }

    unsafe {
        if afficher {
            ANativeActivity_showSoftInput(activity_ptr, 2); // 2 = SHOW_FORCED
        } else {
            ANativeActivity_hideSoftInput(activity_ptr, 0); // 0 = HIDE_IMPLICIT_ONLY
        }
    }
}

#[cfg(not(target_os = "android"))]
pub fn forcer_clavier_android(_afficher: bool) {}