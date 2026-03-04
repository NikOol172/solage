// solage_core/src/auth.rs

#[derive(Debug, Clone)]
pub enum AuthState {
    LoggedOut,
    Pending,
    LoggedIn { username: String, token: String },
    Failed(String),
}

pub trait AuthProvider {
    fn state(&self) -> &AuthState;
    fn login(&mut self, username: &str, password: &str, ctx: &egui::Context);
    fn logout(&mut self);
    fn poll(&mut self) {}  // Implémentation vide par défaut
    fn base_url(&self) -> Option<&str> { None }
    
    fn is_ready(&self) -> bool {
        matches!(self.state(), AuthState::LoggedIn { .. })
    }
    
    fn username(&self) -> Option<&str> {
        match self.state() {
            AuthState::LoggedIn { username, .. } => Some(username),
            _ => None,
        }
    }
    
    fn token(&self) -> Option<&str> {
        match self.state() {
            AuthState::LoggedIn { token, .. } => Some(token),
            _ => None,
        }
    }
    
    fn error_message(&self) -> Option<&str> {
        match self.state() {
            AuthState::Failed(msg) => Some(msg),
            _ => None,
        }
    }
}

pub struct NoAuth {
    state: AuthState,
}

impl NoAuth {
    pub fn new() -> Self {
        Self {
            state: AuthState::LoggedIn {
                username: "local".to_string(),
                token: String::new(),
            }
        }
    }
}

impl Default for NoAuth {
    fn default() -> Self { Self::new() }
}

impl AuthProvider for NoAuth {
    fn state(&self) -> &AuthState { &self.state }
    fn login(&mut self, _u: &str, _p: &str, _ctx: &egui::Context) {}
    fn logout(&mut self) {}
    fn is_ready(&self) -> bool { true }
}
