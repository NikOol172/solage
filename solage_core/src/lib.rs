// solage_core/src/lib.rs

pub mod script;
pub use script::{ScriptEngine, ScriptContext};

pub mod auth;
pub mod platform;
pub use auth::{AuthProvider, AuthState, NoAuth};
pub use solage_data::*;
pub use platform::PlatformBackend;


pub mod config;
pub use config::load_config;

pub mod state;
pub use state::{
    load_state,
    save_state,
    load_preferences,
    save_preferences,
};


