use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// 1. LA RACINE (AppConfig)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default)]
    pub title: String,

    #[serde(default)]
    pub version: String,

    #[serde(default)]
    pub actions: Vec<Action>,

    #[serde(default)]
    pub sections: Vec<Section>,
}

// 2. LES ACTIONS (Scripts)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub label: String,
    pub script: String,
}

// 3. NIVEAU 1 : SECTION (ex: Global Settings, Assets)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub name: String,
    pub icon: String,
    pub modes: Vec<Mode>, // Nouvelle couche !
}

// 4. NIVEAU 2 : MODE (ex: General, Modeling, Surfacing)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mode {
    pub name: String,
    pub flavors: Vec<Flavor>,
}

// 5. NIVEAU 3 : FLAVOR (ex: Default, Prop, Character)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowDef {
    pub key: String,
    pub label: String,
    pub widget: WidgetDef,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub name: String,
    #[serde(default)]
    pub values: HashMap<String, String>,
    #[serde(default)]
    pub computes: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flavor {
    pub name: String,
    pub row_definitions: Vec<RowDef>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

// 6. NIVEAU 4 : ROW (La ligne d'interface)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub key: String,   // ID unique pour le script (ex: "show_name")
    pub label: String, // Texte affiché (ex: "Show Name")
    pub widget: WidgetDef,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum WidgetType {
    #[default]
    Text,
    Number,
    Bool,
    Checkbox,
    Dropdown,
    Path,
    Slider,
}

// 7. LE WIDGET (Définition et Valeur)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetDef {

    #[serde(rename = "type")]
    pub widget_type: WidgetType,

    pub default: Option<serde_json::Value>, 
    
    #[serde(skip)] 
    pub value: Option<String>,

    // Options spécifiques
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub validation: Option<String>,
    pub compute: Option<String>,
    pub options: Option<Vec<String>>,
    pub directory: Option<bool>,
    pub speed: Option<f32>,
    pub speed_shift: Option<f32>,
    pub precision: Option<usize>,
}

impl WidgetDef {
    pub fn validation_rule(&self) -> Option<&str> {
        match self.widget_type {
            WidgetType::Text => {
                self.validation.as_deref()
            }
            _ => None, 
        }
    }

    pub fn compute_rule(&self) -> Option<&str> {
        self.compute.as_deref()
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct GlobalPreferences {
    pub recent_files: Vec<std::path::PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppState {
    pub nav: NavState,
    pub user_values: std::collections::HashMap<String, String>,
    pub prefs: GlobalPreferences,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            nav: NavState::default(),
            user_values: std::collections::HashMap::new(),
            prefs: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavState {
    pub section: usize,
    pub mode: usize,
    pub flavor: usize,
    pub step: usize,
}
