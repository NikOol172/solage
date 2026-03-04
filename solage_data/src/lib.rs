use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// 1. LA RACINE (AppConfig)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
pub struct Action {
    pub label: String,
    pub script: String,
}

// 3. NIVEAU 1 : SECTION (ex: Global Settings, Assets)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    pub icon: String,
    pub modes: Vec<Mode>, // Nouvelle couche !
}

// 4. NIVEAU 2 : MODE (ex: General, Modeling, Surfacing)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mode {
    pub name: String,
    pub flavors: Vec<Flavor>,
}

// 5. NIVEAU 3 : FLAVOR (ex: Default, Prop, Character)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RowDef {
    pub key: String,
    pub label: String,
    pub widget: WidgetDef,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub values: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Flavor {
    pub name: String,
    pub row_definitions: Vec<RowDef>,
    pub steps: Vec<Step>,
}

// 6. NIVEAU 4 : ROW (La ligne d'interface)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Row {
    pub key: String,   // ID unique pour le script (ex: "show_name")
    pub label: String, // Texte affiché (ex: "Show Name")
    pub widget: WidgetDef,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
pub struct WidgetDef {

    #[serde(rename = "type")]
    pub widget_type: WidgetType,

    // Configuration YAML (Polyvalent : String, Number, Bool)
    // On utilise serde_json::Value pour accepter n'importe quoi du YAML
    pub default: Option<serde_json::Value>, 
    
    // État Interne (Ce que l'UI modifie)
    // Ce champ n'est pas dans le YAML, on l'ignorera au chargement (skip)
    #[serde(skip)] 
    pub value: Option<String>,

    // Options spécifiques
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub validation: Option<String>,
    pub compute: Option<String>,
    pub options: Option<Vec<String>>, // Pour les dropdowns
    pub directory: Option<bool>,      // Pour les path
}

impl WidgetDef {
    pub fn validation_rule(&self) -> Option<&str> {
        // CORRECTION 1 : On utilise 'self' au lieu de 'widget'
        match self.widget_type {
            WidgetType::Text => {
                // On accède au champ validation de la structure
                self.validation.as_deref()
            }
            // Pour les sliders ou autres, pas de validation (ou à adapter)
            _ => None, 
        }
    }

    pub fn compute_rule(&self) -> Option<&str> {
        // CORRECTION 2 : Plus besoin de "match" sur les types !
        // Comme WidgetDef est une structure, le champ 'compute' est
        // disponible pour tout le monde directement.
        self.compute.as_deref()
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct GlobalPreferences {
    pub recent_files: Vec<std::path::PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    pub config: AppConfig,
    pub nav: NavState,
    pub prefs: GlobalPreferences,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NavState {
    pub section: usize,
    pub mode: usize,
    pub flavor: usize,
    pub step: usize,    // ← NOUVEAU
}
