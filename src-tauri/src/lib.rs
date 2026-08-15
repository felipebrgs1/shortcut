mod apps;
mod clipboard;
mod config;
mod search;

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use search::SearchResult;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};

static SUPPRESS_BLUR: AtomicBool = AtomicBool::new(false);
/// Visibilidade rastreada manualmente: no Wayland, `is_visible()` do winit/wry
/// não é confiável e o toggle mostraria em vez de esconder.
static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(false);

#[derive(Serialize)]
struct ConfigView {
    can_paste: bool,
    max_history: usize,
    save_images: bool,
}

#[tauri::command]
fn get_config() -> ConfigView {
    let cfg = config::CONFIG.lock();
    ConfigView {
        can_paste: clipboard::which("wtype").is_some(),
        max_history: cfg.max_history,
        save_images: cfg.save_images,
    }
}

#[tauri::command]
fn set_config(max_history: usize, save_images: bool) {
    let mut cfg = config::CONFIG.lock();
    cfg.max_history = max_history.clamp(10, 1000);
    cfg.save_images = save_images;
    cfg.save();
    drop(cfg);
    clipboard::HISTORY.lock().apply_limit();
}

#[tauri::command]
fn clear_history() {
    clipboard::HISTORY.lock().clear();
}

#[derive(Serialize)]
struct HistoryStats {
    n: usize,
    images: usize,
}

#[tauri::command]
fn history_stats() -> HistoryStats {
    let h = clipboard::HISTORY.lock();
    HistoryStats {
        n: h.entries.len(),
        images: h.count_images(),
    }
}

#[tauri::command]
fn hide_window(app: AppHandle) {
    hide_main(&app);
}

#[tauri::command]
fn search_cmd(query: &str) -> Vec<SearchResult> {
    let q = query.trim();
    let matcher = SkimMatcherV2::default();
    let mut out: Vec<SearchResult> = Vec::new();

    if let Some(value) = search::calc_result(q) {
        out.push(SearchResult {
            kind: "calc".into(),
            title: format!("= {value}"),
            subtitle: "Copiar resultado".into(),
            icon: None,
            score: 0,
            data: value,
        });
    }

    if q.is_empty() {
        // sem busca: apenas histórico
    } else if matches_settings(q) {
        out.push(SearchResult {
            kind: "settings".into(),
            title: "Configurações".into(),
            subtitle: "Atalho global, histórico, imagens".into(),
            icon: None,
            score: i64::MAX - 1,
            data: String::new(),
        });
    }

    if !q.is_empty() {
        let mut scored: Vec<(i64, &apps::AppEntry)> = apps::APPS
            .iter()
            .filter_map(|a| matcher.fuzzy_match(&a.name, q).map(|s| (s, a)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        for (score, app) in scored.into_iter().take(8) {
            out.push(SearchResult {
                kind: "app".into(),
                title: app.name.clone(),
                subtitle: app.comment.clone().unwrap_or_default(),
                icon: app.icon.as_deref().and_then(apps::resolve_icon_cached),
                score,
                data: app.id.clone(),
            });
        }
    }

    let hist = clipboard::HISTORY.lock();
    let mut cscored: Vec<(i64, &clipboard::ClipEntry)> = if q.is_empty() {
        hist.entries.iter().take(10).map(|e| (0, e)).collect()
    } else {
        hist.entries
            .iter()
            .filter_map(|e| matcher.fuzzy_match(&e.text, q).map(|s| (s, e)))
            .collect()
    };
    if !q.is_empty() {
        cscored.sort_by(|a, b| b.0.cmp(&a.0));
    }
    for (score, entry) in cscored.into_iter().take(8) {
        let kind = entry.kind.clone();
        let (title, subtitle) = match kind.as_str() {
            "image" => ("Imagem do clipboard".to_string(), search::rel_time(entry.ts)),
            "file" => file_display(&entry.text),
            _ => (search::one_line(&entry.text), search::rel_time(entry.ts)),
        };
        let icon = if kind == "image" { entry.thumb.clone() } else { None };
        out.push(SearchResult {
            kind,
            title,
            subtitle,
            icon,
            score,
            data: entry.text.clone(),
        });
    }

    out
}

fn matches_settings(q: &str) -> bool {
    if q.len() > 40 {
        return false;
    }
    const KW: [&str; 8] = [
        "config", "configurac", "settings", "shortcut", "atalho", "ajust", "preferen",
        "histor",
    ];
    let n = normalize(q);
    KW.iter().any(|k| n.contains(k))
}

fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(|c| c.to_lowercase()) {
        out.push(match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            other => other,
        });
    }
    out
}

fn file_display(uris: &str) -> (String, String) {
    let mut names: Vec<String> = uris
        .lines()
        .filter_map(|l| l.trim().strip_prefix("file://"))
        .map(|p| {
            std::path::Path::new(p.trim_end_matches('/'))
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        return ("Arquivo".into(), "Copiado".into());
    }
    let total = names.len();
    let shown = if total > 3 {
        let extra = total - 2;
        let mut s = names.drain(..2).collect::<Vec<_>>().join(", ");
        s.push_str(&format!(" e mais {extra}"));
        s
    } else {
        names.join(", ")
    };
    let sub = if total == 1 {
        "1 arquivo".into()
    } else {
        format!("{total} arquivos")
    };
    (shown, sub)
}

#[tauri::command]
fn execute(app: AppHandle, result: SearchResult) {
    let mut paste_after = false;
    match result.kind.as_str() {
        "app" => {
            if let Err(e) = apps::launch(&result.data) {
                eprintln!("shortcut: {e}");
            }
        }
        "text" | "file" => {
            clipboard::set_clipboard(&result.data);
            paste_after = true;
        }
        "image" => {
            clipboard::paste_image(&result.data);
            paste_after = true;
        }
        "calc" => clipboard::set_clipboard(&result.data),
        _ => {}
    }
    hide_main(&app);
    if paste_after {
        clipboard::maybe_paste();
    }
}

fn hide_main(app: &AppHandle) {
    WINDOW_VISIBLE.store(false, Ordering::Relaxed);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

fn show_main(app: &AppHandle) {
    WINDOW_VISIBLE.store(true, Ordering::Relaxed);
    if let Some(w) = app.get_webview_window("main") {
        SUPPRESS_BLUR.store(true, Ordering::Relaxed);
        let _ = w.show();
        let _ = w.center();
        let _ = w.set_focus();
        let _ = app.emit("window-shown", ());
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            SUPPRESS_BLUR.store(false, Ordering::Relaxed);
        });
    }
}

fn toggle_main(app: &AppHandle) {
    if WINDOW_VISIBLE.load(Ordering::Relaxed) {
        hide_main(app);
    } else {
        show_main(app);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            toggle_main(app);
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    // Só no evento Pressed (senão o release repetiria o toggle).
                    if event.state()
                        == tauri_plugin_global_shortcut::ShortcutState::Pressed
                        && shortcut.matches(
                            tauri_plugin_global_shortcut::Modifiers::ALT,
                            tauri_plugin_global_shortcut::Code::Space,
                        )
                    {
                        toggle_main(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            clipboard::start_watcher(move || {
                let _ = handle.emit("clipboard-changed", ());
            });
            std::sync::LazyLock::force(&apps::APPS);
            // Best-effort no X11; Wayland registra e pode falhar silenciosamente.
            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            if let Err(e) = app.global_shortcut().register("Alt+Space") {
                eprintln!("shortcut: atalho global indisponível: {e}");
            }
            show_main(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(false) = event {
                if !SUPPRESS_BLUR.load(Ordering::Relaxed) {
                    WINDOW_VISIBLE.store(false, Ordering::Relaxed);
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            search_cmd,
            execute,
            get_config,
            set_config,
            clear_history,
            history_stats,
            hide_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
