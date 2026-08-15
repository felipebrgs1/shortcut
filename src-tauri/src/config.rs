use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::LazyLock;

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    /// Máximo de itens salvos no histórico do clipboard.
    pub max_history: usize,
    /// Salvar imagens copiadas no histórico.
    pub save_images: bool,
    /// Atalho global habilitado?
    pub hotkey_enabled: bool,
    /// Atalho global pedido ao portal (formato Qt, ex: "Alt+Space").
    pub shortcut: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_history: 300,
            save_images: true,
            hotkey_enabled: true,
            shortcut: Some("Alt+Space".into()),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        let dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        dir.join("shortcut/config.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}

pub static CONFIG: LazyLock<Mutex<Config>> = LazyLock::new(|| Mutex::new(Config::load()));