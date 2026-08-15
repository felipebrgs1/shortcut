use arboard::Clipboard;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;

const MAX_ENTRIES: usize = 300;
const MAX_TEXT_LEN: usize = 20_000;

#[derive(Clone, Serialize, Deserialize)]
pub struct ClipEntry {
    pub text: String,
    pub ts: u64,
}

pub struct ClipboardHistory {
    pub entries: Vec<ClipEntry>,
    pub last_set: Option<String>,
    path: PathBuf,
}

impl ClipboardHistory {
    fn load() -> Self {
        let dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("shortcut");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("clipboard.json");
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            entries,
            last_set: None,
            path,
        }
    }

    fn add(&mut self, text: String) -> bool {
        let text = text.trim_end().to_string();
        if text.is_empty() || text.len() > MAX_TEXT_LEN {
            return false;
        }
        if self.last_set.as_deref() == Some(text.as_str()) {
            self.last_set = None;
            return false;
        }
        self.entries.retain(|e| e.text != text);
        self.entries.insert(
            0,
            ClipEntry {
                text,
                ts: now_millis(),
            },
        );
        self.entries.truncate(MAX_ENTRIES);
        self.save();
        true
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_string(&self.entries) {
            let _ = std::fs::write(&self.path, json);
        }
    }
}

pub static HISTORY: LazyLock<Mutex<ClipboardHistory>> =
    LazyLock::new(|| Mutex::new(ClipboardHistory::load()));

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn start_watcher(on_change: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let mut cb = match Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("clipboard watcher: falha ao iniciar: {e}");
                return;
            }
        };
        loop {
            if let Ok(text) = cb.get_text() {
                let added = {
                    let mut h = HISTORY.lock();
                    let is_latest = h
                        .entries
                        .first()
                        .map(|e| e.text == text)
                        .unwrap_or(false);
                    !is_latest && h.add(text)
                };
                if added {
                    on_change();
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(600));
        }
    });
}

pub fn set_clipboard(text: &str) {
    HISTORY.lock().last_set = Some(text.to_string());
    if let Some(qdbus) = which("qdbus6").or_else(|| which("qdbus")) {
        if Command::new(qdbus)
            .args([
                "org.kde.klipper",
                "/klipper",
                "org.kde.klipper.klipper.setClipboardContents",
                text,
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return;
        }
    }
    if let Some(wl_copy) = which("wl-copy") {
        if Command::new(wl_copy)
            .arg(text)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return;
        }
    }
    if let Ok(mut cb) = Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }
}

/// Simula Ctrl+V após colar no clipboard (usado no Wayland, onde não dá
/// para injetar teclas programaticamente). Requer `wtype` instalado.
pub fn maybe_paste() {
    let Some(wtype) = which("wtype") else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(160));
        let _ = std::process::Command::new(wtype)
            .args(["-M", "ctrl", "-k", "v", "-m", "ctrl"])
            .status();
    });
}

pub fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| p.is_file())
    })
}
