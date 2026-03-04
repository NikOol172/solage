use egui::{Ui, Color32, RichText, Visuals, Style, TextStyle, Stroke};
use egui_extras::{TableBuilder, Column};
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};

use solage_data::{AppConfig, WidgetDef, Section, Flavor, AppState, GlobalPreferences, WidgetType as SolageWidget}; 
use solage_core::{ScriptEngine, PlatformBackend, ScriptContext, AuthProvider, AuthState, NoAuth, load_config, load_state, save_state, save_preferences, load_preferences};

mod viewer_3d;
pub use viewer_3d::SceneCache;


#[derive(Default)]
pub struct NavigationState {
    pub active_section_idx: usize,
    pub active_mode_indices: HashMap<usize, usize>, 
    pub active_flavor_indices: HashMap<(usize, usize), usize>, 
}

impl NavigationState {
    pub fn new() -> Self {
        Self {
            active_section_idx: 0,
            active_mode_indices: HashMap::new(),
            active_flavor_indices: HashMap::new(),
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

pub struct SolageApp {
    pub backend: Box<dyn PlatformBackend>,
    pub state: AppState,
    pub engine: ScriptEngine,
    pub scene_cache: SceneCache,
    pub preferences: GlobalPreferences,
    config: Option<AppConfig>, 
    current_config_path: Option<PathBuf>,
    nav_state: NavigationState,
    error_msg: Option<String>,
    prefs_path: String,
    login_form: LoginForm,
    auth: Box<dyn AuthProvider>,
    pub url_input: String,
    pub download_rx: Option<Receiver<Result<String, String>>>,
    pub toast: Option<(String, f64)>,
    pub theme_applied: bool,
}

impl SolageApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>, 
        backend: Box<dyn PlatformBackend>,
        auth: Box<dyn AuthProvider>,
    ) -> Self {
        // 1. INSTALLATION DES LOADER D'IMAGES (CRUCIAL)
        egui_extras::install_image_loaders(&cc.egui_ctx);

        // 2. Chargement des préférences
        let prefs_path = backend.get_config_dir().join("user_prefs.json");
        let preferences = load_preferences(prefs_path.to_str().unwrap_or("user_prefs.json"))
            .unwrap_or_default();

        let mut app = Self {
            backend,
            auth,
            state: AppState::default(),
            engine: ScriptEngine::new(),
            scene_cache: SceneCache::new(),
            preferences,
            nav_state: NavigationState::default(),
            login_form: LoginForm::default(),
            config: None,
            current_config_path: None,
            error_msg: None,
            prefs_path: prefs_path.to_string_lossy().to_string(),
            url_input: "https://vacarmesvisuels.com/solage/configs/config.yaml".to_string(), // Mettez votre URL par défaut ici
            download_rx: None,
            toast: None,
            theme_applied: false,
        };

        // #[cfg(target_os = "android")]
        // {
        //     app.load_yaml_string(ANDROID_DEMO_YAML);
        // }
        app
    }

    pub fn load_yaml_string(&mut self, yaml_content: &str) {
        match load_config(yaml_content) {
            Ok(config) => apply_defaults(&config, &mut self.state),
            Err(e) => self.error_msg = Some(format!("Erreur YAML: {}", e)),
        }
    }

    // Chargement robuste avec capture d'erreurs
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

    // Nouvelle fonction pour lancer un téléchargement proprement
    fn fetch_url(&mut self, url: &str, ctx: &egui::Context) {
        self.url_input = url.to_string(); // Met à jour le champ texte
        let (tx, rx) = std::sync::mpsc::channel();
        self.download_rx = Some(rx);
        let ctx_clone = ctx.clone();
        
        let body = serde_json::json!({
            "username": self.login_form.username,
            "password": self.login_form.password
        }).to_string();

        let request = ehttp::Request::post(url, body.into_bytes());
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

}

impl eframe::App for SolageApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        
        // NOUVEAU : Application du thème au premier lancement
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
                            // 1. On sauvegarde dans les récents
                            let url_path = PathBuf::from(&self.url_input);
                            self.add_to_recents(url_path.clone());
                            
                            // 2. CRUCIAL : On dit à l'app que ce fichier est notre fichier "actuel"
                            self.current_config_path = Some(url_path);
                            
                            // 3. On tente de restaurer les variables (sliders, cases cochées...)
                            // depuis le LocalStorage en utilisant l'URL comme clé.
                            if let Ok(saved_state) = load_state(&self.url_input) {
                                self.state = saved_state;
                            }
                        }
                    },
                    Err(e) => self.error_msg = Some(e),
                }
            }
        }

        // Poll auth (vérifie si une réponse est arrivée)
        self.auth.poll();

        // Affiche le toast de bienvenue au login
        if let AuthState::LoggedIn { username, .. } = self.auth.state() {
            if self.toast.is_none() {
                // Premier frame après login
            }
        }

        // Écran d'accueil conditionnel sur l'auth
        if !self.auth.is_ready() {
            self.draw_login_screen(ctx);
            return;
        }

        // --- SPLASH SCREEN (Desktop) ---
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

        // --- 1. MODE "ACCUEIL" (Si aucune config chargée) ---
        if self.state.config.sections.is_empty() {
            egui::CentralPanel::default().show(ctx, |ui| {
                
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
                    ui.label(RichText::new("Pipeline Manager & AMS").size(20.0).color(Color32::GRAY));
                });
                ui.add_space(40.0);

                // --- DIVISION EN DEUX COLONNES ---
                ui.columns(2, |columns| {
                    
                    // ==========================================
                    // COLONNE GAUCHE : CONNEXION STUDIO (AMS)
                    // ==========================================
                    columns[0].vertical_centered(|ui| {
                        ui.heading(RichText::new("🏢 Connexion Studio").strong());
                        ui.add_space(20.0);
                        
                        ui.group(|ui| {
                            ui.set_width(300.0);
                            ui.add_space(10.0);
                            
                            ui.horizontal(|ui| {
                                ui.label("Utilisateur:     ");
                                ui.add(egui::TextEdit::singleline(&mut self.login_form.username).desired_width(200.0));
                            });
                            ui.add_space(5.0);
                            ui.horizontal(|ui| {
                                ui.label("Mot de passe:");
                                ui.add(egui::TextEdit::singleline(&mut self.login_form.password).password(true).desired_width(200.0));
                            });
                            
                            ui.add_space(15.0);
                            
                            
                            
                            // if let Some(msg) = &self.login_msg {
                            //     ui.add_space(10.0);
                            //     ui.colored_label(Color32::LIGHT_BLUE, msg);
                            // }
                            ui.add_space(10.0);
                        });
                    });

                    // ==========================================
                    // COLONNE DROITE : MODE AUTONOME / VERSATILE
                    // ==========================================
                    columns[1].vertical_centered(|ui| {
                        ui.heading(RichText::new("🛠️ Mode Autonome").strong());
                        ui.add_space(20.0);
                        
                        // 1. Ouvrir Fichier Local
                        let btn = egui::Button::new(RichText::new("📂 Ouvrir fichier local...").size(16.0))
                            .min_size(egui::vec2(300.0, 35.0));
                        if ui.add(btn).clicked() {
                            if let Some(path) = self.backend.pick_file() {
                                self.load_config_from_path(&path);
                            }
                        }

                        // 2. Ouvrir URL Custom
                        ui.add_space(15.0);
                        ui.horizontal(|ui| {
                            ui.add_space(ui.available_width() / 2.0 - 150.0); // Centrage manuel
                            ui.add(egui::TextEdit::singleline(&mut self.url_input).min_size(egui::vec2(220.0, 30.0)));
                            if ui.add(egui::Button::new("⬇ URL").min_size(egui::vec2(70.0, 30.0))).clicked() {
                                let url = self.url_input.clone();
                                self.fetch_url(&url, ctx);
                            }
                        });

                        // 3. Fichiers Récents
                        if !self.preferences.recent_files.is_empty() {
                            ui.add_space(30.0);
                            ui.separator();
                            ui.add_space(10.0);
                            ui.label(RichText::new("RÉCEMMENT OUVERTS").size(12.0).strong().color(Color32::GRAY));
                            ui.add_space(10.0);

                            // LA CORRECTION MAGIQUE EST ICI 👇
                            // On clone la liste pour "libérer" self des emprunts immuables !
                            let recent_files = self.preferences.recent_files.clone();

                            for path in recent_files {
                                let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                                let full_path = path.to_string_lossy();
                                
                                ui.scope(|ui| {
                                    ui.style_mut().visuals.widgets.inactive.weak_bg_fill = Color32::from_gray(30);
                                    let btn = egui::Button::new(RichText::new(format!("📄 {}", file_name)).size(14.0))
                                        .min_size(egui::vec2(280.0, 28.0)).frame(true);

                                    if ui.add(btn).on_hover_text(full_path.to_string()).clicked() {
                                        let path_str = full_path.to_string();
                                        if path_str.starts_with("http") {
                                            self.fetch_url(&path_str, ctx);
                                        } else {
                                            self.load_config_from_path(&path.clone()); // On clone le path aussi par sécurité
                                        }
                                    }
                                });
                                ui.add_space(5.0);
                            }
                        }
                    });
                });
            });

            // Overlay splash finissant
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

        // --- 2. INTERFACE PRINCIPALE ---

        // --- NOUVEAU : RACCOURCI CLAVIER (Ctrl+S / Cmd+S) ---
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
        
        // Optimisation Context
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
                    
                    // --- BOUTON FERMER (AVEC AUTO-SAVE) ---
                    if ui.button("Fermer Projet").clicked() {
                        if let Some(path) = &self.current_config_path {
                            let path_str = path.to_string_lossy().to_string();
                            // Si c'est une URL on utilise l'URL, sinon on crée un fichier .json
                            let state_path = if path_str.starts_with("http") {
                                path_str
                            } else {
                                path.with_extension("json").to_string_lossy().to_string()
                            };
                            // On sauvegarde automatiquement !
                            let _ = save_state(&state_path, &self.state);
                        }
                        
                        // Puis on nettoie l'interface pour revenir à l'accueil
                        self.state.config.sections.clear();
                        self.current_config_path = None;
                    }

                    // --- BOUTON SAUVEGARDER MANUEL ---
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
                if let Some(path) = self.backend.pick_file() {
                    self.load_config_from_path(&path);
                }
            }
        });

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
                        draw_comparison_table(ui, active_flavor, &mut script_context, &self.engine, &self.backend);
                    }
                }
            }
        });
        
        if let Some((msg, start_time)) = &self.toast {
            let current_time = ctx.input(|i| i.time);
            let elapsed = current_time - start_time;
            
            // On l'affiche pendant 2.5 secondes
            if elapsed < 2.5 {
                // Calcul du fondu (fade out) sur la dernière demi-seconde
                let alpha = if elapsed > 2.0 { 1.0 - ((elapsed - 2.0) * 2.0) as f32 } else { 1.0 };
                
                egui::Area::new(egui::Id::new("toast_area"))
                    .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0)) // En bas au centre
                    .order(egui::Order::Foreground) // Toujours au premier plan
                    .show(ctx, |ui| {
                        // Un joli cadre sombre aux bords arrondis
                        egui::Frame::popup(ui.style())
                            .fill(Color32::from_black_alpha((200.0 * alpha) as u8))
                            .stroke(egui::Stroke::NONE)
                            .rounding(8.0)
                            .inner_margin(12.0)
                            .show(ui, |ui| {
                                ui.label(RichText::new(msg).color(Color32::from_white_alpha((255.0 * alpha) as u8)).strong().size(16.0));
                            });
                    });
                
                // On demande à egui de redessiner l'écran à la prochaine frame pour que l'animation soit fluide
                ctx.request_repaint();
            } else {
                // Le temps est écoulé, on supprime le toast
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

// --- HELPER FUNCTIONS ---


fn draw_splash_anim(ctx: &egui::Context, time: f64) {
    let fade_out = if time > 1.5 { (2.5 - time).clamp(0.0, 1.0) as f32 } else { 1.0 };
    
    egui::CentralPanel::default().show(ctx, |ui| {
        let rect = ui.max_rect();
        ui.painter().rect_filled(
            rect, 
            egui::Rounding::ZERO, 
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

fn draw_comparison_table(
    ui: &mut Ui, 
    flavor: &mut Flavor, 
    script_context: &mut ScriptContext, 
    engine: &ScriptEngine, 
    backend: &Box<dyn PlatformBackend>
) {
    if flavor.steps.is_empty() {
        ui.colored_label(Color32::GRAY, "Aucun step défini");
        return;
    }

    // Construction dynamique des colonnes
    let mut builder = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .column(Column::initial(150.0)); // Colonne "Label"

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
                                    // Valeur par défaut si absente
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

                            draw_cell_value(ui, value, &row_def.widget, engine, backend);
                        });
                    }
                });
            }
        });
}

fn draw_cell_widget(ui: &mut Ui, widget: &mut WidgetDef, ctx: &mut ScriptContext, engine: &ScriptEngine, backend: &Box<dyn PlatformBackend>) {
    if widget.value.is_none() { widget.value = Some("".to_string()); }
    if let Some(rule) = widget.compute_rule() {
        if let Some(result) = engine.eval_with_context(rule, ctx, widget.value.as_deref()) {
            widget.value = Some(result);
        }
    }
    let value_ref = widget.value.as_mut().unwrap();
    match widget.widget_type {
        SolageWidget::Text => { ui.text_edit_singleline(value_ref); },
        SolageWidget::Number => {
            if let Ok(mut num) = value_ref.parse::<f32>() {
                if ui.add(egui::DragValue::new(&mut num)).changed() { *value_ref = num.to_string(); }
            } else { ui.text_edit_singleline(value_ref); }
        },
        SolageWidget::Slider => {
            if let Ok(mut num) = value_ref.parse::<f32>() {
                let min = widget.min.unwrap_or(0.0);
                let max = widget.max.unwrap_or(100.0);
                if ui.add(egui::Slider::new(&mut num, min..=max)).changed() { *value_ref = num.to_string(); }
            }
        },
        SolageWidget::Bool | SolageWidget::Checkbox => {
            let mut b = value_ref.parse::<bool>().unwrap_or(false);
            if ui.checkbox(&mut b, "").changed() { *value_ref = b.to_string(); }
        },
        SolageWidget::Path => {
            ui.horizontal(|ui| {
                ui.text_edit_singleline(value_ref);
                if ui.button("📂").clicked() {
                    if let Some(p) = backend.pick_file() { *value_ref = p.display().to_string(); }
                }
            });
        },
        SolageWidget::Dropdown => {
            if let Some(options) = &widget.options {
                let options = options.clone();
                egui::ComboBox::from_id_salt("dropdown")
                    .selected_text(value_ref.as_str())
                    .show_ui(ui, |ui| {
                        for opt in &options {
                            ui.selectable_value(value_ref, opt.clone(), opt);
                        }
                    });
            }
        },
        _ => { ui.label(value_ref.as_str()); }
    }
}

fn draw_cell_value(
    ui: &mut Ui,
    value: &mut String,
    widget: &WidgetDef,
    engine: &ScriptEngine,
    backend: &Box<dyn PlatformBackend>,
) {
    match widget.widget_type {
        SolageWidget::Text => { ui.text_edit_singleline(value); },
        SolageWidget::Number => {
            if let Ok(mut num) = value.parse::<f32>() {
                if ui.add(egui::DragValue::new(&mut num)).changed() {
                    *value = num.to_string();
                }
            } else {
                ui.text_edit_singleline(value);
            }
        },
        SolageWidget::Slider => {
            if let Ok(mut num) = value.parse::<f32>() {
                let min = widget.min.unwrap_or(0.0);
                let max = widget.max.unwrap_or(100.0);
                if ui.add(egui::Slider::new(&mut num, min..=max)).changed() {
                    *value = num.to_string();
                }
            }
        },
        SolageWidget::Bool => {
            let mut b = value.parse::<bool>().unwrap_or(false);
            if ui.checkbox(&mut b, "").changed() {
                *value = b.to_string();
            }
        },
        SolageWidget::Path => {
            ui.horizontal(|ui| {
                ui.text_edit_singleline(value);
                if ui.button("📂").clicked() {
                    if let Some(p) = backend.pick_file() {
                        *value = p.display().to_string();
                    }
                }
            });
        },
        SolageWidget::Dropdown => {
            if let Some(options) = &widget.options {
                let options = options.clone();
                egui::ComboBox::from_id_salt(&widget as *const _ as usize)
                    .selected_text(value.as_str())
                    .show_ui(ui, |ui| {
                        for opt in &options {
                            ui.selectable_value(value, opt.clone(), opt);
                        }
                    });
            }
        },
        _ => { ui.label(value.as_str()); }
    }
}

fn apply_defaults(config: &AppConfig, state: &mut AppState) {
    state.config = config.clone();
    state.nav.section = 0;  // ← plus current_section_idx
    state.nav.mode = 0;
    state.nav.flavor = 0;
    // for section in &mut state.config.sections {
    //     for mode in &mut section.modes {
    //         for flavor in &mut mode.flavors {
    //             for step in &mut flavor.steps {
    //                 for row in &mut step.rows {
    //                     if row.widget.value.is_none() {
    //                         if let Some(def) = &row.widget.default {
    //                             let s = match def {
    //                                 serde_json::Value::String(s) => s.clone(),
    //                                 serde_json::Value::Number(n) => n.to_string(),
    //                                 serde_json::Value::Bool(b) => b.to_string(),
    //                                 _ => String::new(),
    //                             };
    //                             row.widget.value = Some(s);
    //                         }
    //                     }
    //                 }
    //             }
    //         }
    //     }
    // }
}

// --- CHARTE GRAPHIQUE SOLAGE ---
pub fn apply_studio_theme(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    
    // On charge le fichier TTF directement dans le binaire compilé
    fonts.font_data.insert(
        "StudioFont".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/font.ttf"))), // ✅ Enveloppé dans un Arc
    );

    // On ordonne à egui d'utiliser cette police en priorité absolue pour le texte proportionnel (normal)
    fonts.families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "StudioFont".to_owned());

    // On applique les nouvelles polices au contexte
    ctx.set_fonts(fonts);

    // 1. On part sur une base sombre
    let mut visuals = Visuals::dark();
    
    // 2. Couleurs de fond (Gris anthracite bleuté, très pro)
    visuals.window_fill = Color32::from_rgb(25, 27, 31);
    visuals.panel_fill = Color32::from_rgb(18, 20, 24);
    
    // 3. Couleur d'accentuation (Le "Bleu Solage" pour les sélections et sliders)
    let solage_blue = Color32::from_rgb(100, 180, 255);
    let dark_text = Color32::from_rgb(20, 22, 25); // Un gris presque noir, très élégant

    visuals.selection.bg_fill = solage_blue;
    // On force le texte à l'intérieur des zones sélectionnées à être sombre
    visuals.selection.stroke = egui::Stroke::new(1.0, dark_text); 
    
    // On s'assure aussi que quand on clique (état "actif"), le texte reste lisible
    visuals.widgets.active.bg_fill = solage_blue;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, dark_text);
    
    // 4. Arrondir les angles (Design plus moderne, moins rigide)
    let radius = egui::CornerRadius::same(6); // On utilise CornerRadius au lieu de Rounding
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.active.corner_radius = radius;
    visuals.window_corner_radius = egui::CornerRadius::same(10); // Les fenêtres flottantes plus arrondies
    
    // 5. Appliquer les couleurs au contexte
    ctx.set_visuals(visuals);

    // 6. Ajustement global des textes (Un peu plus grands et lisibles)
    let mut style = (*ctx.style()).clone();
    
    style.text_styles.insert(TextStyle::Body, egui::FontId::proportional(15.0));
    style.text_styles.insert(TextStyle::Button, egui::FontId::proportional(15.0));
    style.text_styles.insert(TextStyle::Monospace, egui::FontId::monospace(14.0));
    style.text_styles.insert(TextStyle::Heading, egui::FontId::proportional(26.0));
    
    // Un peu plus d'espace entre les éléments
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    
    // Appliquer le style
    ctx.set_style(style);
}
