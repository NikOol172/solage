// solage_core/src/config.rs

use crate::AppConfig;

pub fn load_config(yaml_content: &str) -> Result<AppConfig, serde_yaml::Error> {
    serde_yaml::from_str(yaml_content)
}
