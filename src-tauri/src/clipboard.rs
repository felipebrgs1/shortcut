use arboard::{Clipboard, ImageData};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;

const MAX_TEXT_LEN: usize = 20_000;
const MAX_IMAGE_PX: u64 = 4096 * 4096;

#[derive(Clone, Serialize, Deserialize)]
pub struct ClipEntry {
    /// "text" | "image" | "file"
    #[serde(default)]
    pub kind: String,
    /// texto | uri-list | caminho do png
    pub text: String,
    #[serde(default)]
    pub thumb: Option<String>,
    pub ts: u64,
    #[serde(default)]
    pub hash: Option<u64>,
}

pub struct ClipboardHistory {
    pub entries: Vec<ClipEntry>,
    pub last_set: Option<String>,
    last_set_image: Option<u64>,
    path: PathBuf,
    images_dir: PathBuf,
}

impl ClipboardHistory {
    fn load() -> Self {
        let dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("shortcut");
        let _ = std::fs::create_dir_all(&dir);
        let images_dir = dir.join("images");
        let _ = std::fs::create_dir_all(&images_dir);
        let _ = std::fs::create_dir_all(images_dir.join("thumbs"));
        let path = dir.join("clipboard.json");
        let mut entries: Vec<ClipEntry> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        for e in &mut entries {
            if e.kind.is_empty() {
                e.kind = "text".into();
            }
        }
        Self {
            entries,
            last_set: None,
            last_set_image: None,
            path,
            images_dir,
        }
    }

    fn max_history(&self) -> usize {
        crate::config::CONFIG.lock().max_history.clamp(10, 1000)
    }

    pub fn add_text(&mut self, text: String) -> bool {
        let text = text.trim_end().to_string();
        if text.is_empty() || text.len() > MAX_TEXT_LEN {
            return false;
        }
        if self.last_set.as_deref() == Some(text.as_str()) {
            self.last_set = None;
            return false;
        }
        self.entries
            .retain(|e| !(e.kind == "text" && e.text == text));
        self.entries.insert(
            0,
            ClipEntry {
                kind: "text".into(),
                text,
                thumb: None,
                ts: now_millis(),
                hash: None,
            },
        );
        self.truncate_save();
        true
    }

    pub fn add_file(&mut self, uris: String) -> bool {
        if self.last_set.as_deref() == Some(uris.as_str()) {
            self.last_set = None;
            return false;
        }
        self.entries
            .retain(|e| !(e.kind == "file" && e.text == uris));
        self.entries.insert(
            0,
            ClipEntry {
                kind: "file".into(),
                text: uris,
                thumb: None,
                ts: now_millis(),
                hash: None,
            },
        );
        self.truncate_save();
        true
    }

    pub fn add_image(&mut self, image: &ImageData) -> Option<String> {
        if !crate::config::CONFIG.lock().save_images {
            return None;
        }
        let (w, h) = (image.width as u64, image.height as u64);
        if w * h > MAX_IMAGE_PX {
            return None;
        }
        if self
            .last_set_image
            .map(|h| h == hash_bytes(&image.bytes))
            .unwrap_or(false)
        {
            self.last_set_image = None;
            return None;
        }
        let hash = hash_bytes(&image.bytes);
        if self
            .entries
            .iter()
            .any(|e| e.kind == "image" && e.hash == Some(hash))
        {
            return None;
        }
        let Some(rgba) = image::RgbaImage::from_raw(
            image.width as u32,
            image.height as u32,
            image.bytes.to_vec(),
        ) else {
            return None;
        };
        let name = format!("{}_{:016x}", now_millis(), hash);
        let full = self.images_dir.join(format!("{name}.png"));
        if rgba.save(&full).is_err() {
            return None;
        }
        let thumb = full
            .parent()
            .map(|d| d.join("thumbs").join(format!("{name}.png")));
        let thumb_str = if let Some(t) = thumb {
            let th = image::imageops::thumbnail(&rgba, 64, 64);
            th.save(&t).ok().map(|_| t.to_string_lossy().into_owned())
        } else {
            None
        };
        let full_str = full.to_string_lossy().into_owned();
        self.entries.insert(
            0,
            ClipEntry {
                kind: "image".into(),
                text: full_str.clone(),
                thumb: thumb_str,
                ts: now_millis(),
                hash: Some(hash),
            },
        );
        self.truncate_save();
        Some(full_str)
    }

    /// Aplica o limite atual, limpa arquivos órfãos e salva.
    pub fn apply_limit(&mut self) {
        self.truncate_save();
    }

    fn truncate_save(&mut self) {
        let max = self.max_history();
        self.entries.truncate(max);
        self.cleanup_images();
        self.save();
    }

    fn cleanup_images(&mut self) {
        let keep: HashSet<String> = self
            .entries
            .iter()
            .filter(|e| e.kind == "image")
            .flat_map(|e| {
                let mut v = vec![e.text.clone()];
                if let Some(t) = &e.thumb {
                    v.push(t.clone());
                }
                v
            })
            .collect();
        for dir in [self.images_dir.clone(), self.images_dir.join("thumbs")] {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for f in rd.flatten() {
                    let p = f.path();
                    if !keep.contains(&p.to_string_lossy().into_owned()) {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        for dir in [self.images_dir.clone(), self.images_dir.join("thumbs")] {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for f in rd.flatten() {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        }
        self.save();
    }

    pub fn count_images(&self) -> usize {
        self.entries.iter().filter(|e| e.kind == "image").count()
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

/// FNV-1a 64 com amostragem (imagens grandes sem pesar).
fn hash_bytes(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b.iter().step_by(16) {
        h ^= x as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn looks_like_uri_list(text: &str) -> bool {
    text.lines().any(|l| l.trim_start().starts_with("file://"))
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
                let is_file = looks_like_uri_list(&text);
                let added = {
                    let mut h = HISTORY.lock();
                    let is_latest = h
                        .entries
                        .first()
                        .map(|e| {
                            e.text == text && (e.kind == "text" || e.kind == "file")
                        })
                        .unwrap_or(false);
                    !is_latest
                        && if is_file {
                            h.add_file(text)
                        } else {
                            h.add_text(text)
                        }
                };
                if added {
                    on_change();
                }
            } else if let Ok(img) = cb.get_image() {
                let added = HISTORY.lock().add_image(&img).is_some();
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
        let mut cmd = Command::new(wl_copy);
        if looks_like_uri_list(text) {
            cmd.args(["--type", "text/uri-list"]);
        }
        if cmd.arg(text).status().map(|s| s.success()).unwrap_or(false) {
            return;
        }
    }
    if let Ok(mut cb) = Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }
}

/// Coloca uma imagem salva (png) de volta no clipboard.
pub fn paste_image(path: &str) {
    let Ok(img) = image::open(path) else {
        return;
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let hash = hash_bytes(&rgba);
    {
        let mut h = HISTORY.lock();
        h.last_set_image = Some(hash);
    }
    let data = ImageData {
        width: w as usize,
        height: h as usize,
        bytes: rgba.into_raw().into(),
    };
    if let Ok(mut cb) = Clipboard::new() {
        let _ = cb.set_image(data);
    }
}

/// Simula Ctrl+V após setar o clipboard (Wayland não permite injetar teclas).
/// Requer `wtype` instalado.
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