use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

#[derive(Clone, Debug, Serialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub icon: Option<String>,
}

pub static APPS: LazyLock<Vec<AppEntry>> = LazyLock::new(scan_apps);

static ICON_CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn resolve_icon_cached(name: &str) -> Option<String> {
    if let Some(v) = ICON_CACHE.lock().get(name) {
        return v.clone();
    }
    let resolved = resolve_icon(name);
    ICON_CACHE.lock().insert(name.to_string(), resolved.clone());
    resolved
}

pub fn launch(id: &str) -> Result<(), String> {
    Command::new("gtk-launch")
        .arg(id)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("gtk-launch falhou: {e}"))
}

fn app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
    ];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
        dirs.push(home.join(".local/share/applications"));
    }
    dirs
}

fn scan_apps() -> Vec<AppEntry> {
    let mut map: HashMap<String, AppEntry> = HashMap::new();
    for dir in app_dirs() {
        visit(&dir, &dir, &mut map);
    }
    let mut apps: Vec<AppEntry> = map.into_values().collect();
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

fn visit(dir: &Path, root: &Path, map: &mut HashMap<String, AppEntry>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, root, map);
        } else if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
            if let Some(app) = parse_desktop(&path, root) {
                map.insert(app.id.clone(), app);
            }
        }
    }
}

fn parse_desktop(path: &Path, root: &Path) -> Option<AppEntry> {
    let content = std::fs::read_to_string(path).ok()?;
    let rel = path.strip_prefix(root).ok()?;
    let id = rel
        .to_string_lossy()
        .trim_end_matches(".desktop")
        .replace('/', "-");

    let mut in_entry = false;
    let mut name = None;
    let mut comment = None;
    let mut icon = None;
    let mut nodisplay = false;
    let mut hidden = false;
    let mut is_application = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "Name" => name = Some(value.to_string()),
            "Comment" => comment = Some(value.to_string()),
            "Icon" => icon = Some(value.to_string()),
            "NoDisplay" => nodisplay = value == "true",
            "Hidden" => hidden = value == "true",
            "Type" => is_application = value == "Application",
            _ => {}
        }
    }

    if nodisplay || hidden || !is_application {
        return None;
    }

    Some(AppEntry {
        id,
        name: name?,
        comment,
        icon,
    })
}

fn current_icon_theme() -> Option<String> {
    let home = dirs::home_dir()?;
    let content = std::fs::read_to_string(home.join(".config/kdeglobals")).ok()?;
    let mut in_icons = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_icons = line == "[Icons]";
            continue;
        }
        if in_icons {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "Theme" {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

fn resolve_icon(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let as_path = Path::new(name);
    if as_path.is_absolute() {
        return as_path.exists().then(|| name.to_string());
    }
    let name = name
        .trim_end_matches(".png")
        .trim_end_matches(".svg")
        .trim_end_matches(".xpm");

    let mut roots = vec![
        PathBuf::from("/usr/share/icons"),
        PathBuf::from("/usr/share/pixmaps"),
        PathBuf::from("/var/lib/flatpak/exports/share/icons"),
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".local/share/icons"));
        roots.push(home.join(".icons"));
    }

    let mut themes: Vec<String> = Vec::new();
    if let Some(t) = current_icon_theme() {
        themes.push(t);
    }
    for t in ["breeze", "breeze-dark", "hicolor", "Adwaita"] {
        if !themes.iter().any(|x| x == t) {
            themes.push(t.to_string());
        }
    }

    let sizes = [
        "scalable", "256x256", "128x128", "96x96", "64x64", "48x48", "32x32", "24x24", "22x22",
        "16x16",
    ];
    let cats = ["apps", "applications", "places", "mimetypes"];
    let exts = ["svg", "png", "xpm"];

    for root in &roots {
        for ext in exts {
            let p = root.join(format!("{name}.{ext}"));
            if p.exists() {
                return Some(p.to_string_lossy().into_owned());
            }
        }
        for theme in &themes {
            if !root.join(theme).is_dir() {
                continue;
            }
            for size in sizes {
                for cat in cats {
                    for ext in exts {
                        let p = root
                            .join(theme)
                            .join(size)
                            .join(cat)
                            .join(format!("{name}.{ext}"));
                        if p.exists() {
                            return Some(p.to_string_lossy().into_owned());
                        }
                        let p = root
                            .join(theme)
                            .join(cat)
                            .join(size)
                            .join(format!("{name}.{ext}"));
                        if p.exists() {
                            return Some(p.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn scans_apps() {
        let apps = super::scan_apps();
        assert!(!apps.is_empty());
    }

    #[test]
    fn resolves_some_icon() {
        let apps = super::scan_apps();
        let with_icon = apps
            .iter()
            .filter_map(|a| a.icon.as_deref())
            .find_map(super::resolve_icon);
        assert!(with_icon.is_some());
    }
}
