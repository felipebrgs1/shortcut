mod apps;
mod clipboard;
mod config;
mod globalshortcut;
mod search;

use eframe::egui;
use egui::{
    Align2, Color32, FontData, FontFamily, FontId, Pos2, Rect, Response, RichText, Sense, Stroke,
    TextEdit, TextureId, Ui, Vec2, ViewportCommand,
};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use search::SearchResult;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const WINDOW_W: f32 = 720.0;
const WINDOW_H: f32 = 464.0;
const ROW_H: f32 = 42.0;
const FOOTER_H: f32 = 34.0;
const RADIUS: f32 = 12.0;

const PANEL: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x24);
const BORDER: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x35);
const TEXT: Color32 = Color32::from_rgb(0xe5, 0xe5, 0xe8);
const DIM: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x93);
const ACCENT: Color32 = Color32::from_rgb(0x7a, 0xa8, 0xff);
const SELECT: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x33);
const HOVER: Color32 = Color32::from_rgb(0x25, 0x25, 0x2c);

static TOGGLE: AtomicBool = AtomicBool::new(false);
static CLIP_DIRTY: AtomicBool = AtomicBool::new(false);
pub(crate) static CTX: OnceLock<egui::Context> = OnceLock::new();

const LETTERS: [egui::Key; 26] = [
    egui::Key::A, egui::Key::B, egui::Key::C, egui::Key::D, egui::Key::E, egui::Key::F,
    egui::Key::G, egui::Key::H, egui::Key::I, egui::Key::J, egui::Key::K, egui::Key::L,
    egui::Key::M, egui::Key::N, egui::Key::O, egui::Key::P, egui::Key::Q, egui::Key::R,
    egui::Key::S, egui::Key::T, egui::Key::U, egui::Key::V, egui::Key::W, egui::Key::X,
    egui::Key::Y, egui::Key::Z,
];
const DIGITS: [egui::Key; 10] = [
    egui::Key::Num0, egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4,
    egui::Key::Num5, egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9,
];
const FN_KEYS: [egui::Key; 12] = [
    egui::Key::F1, egui::Key::F2, egui::Key::F3, egui::Key::F4, egui::Key::F5, egui::Key::F6,
    egui::Key::F7, egui::Key::F8, egui::Key::F9, egui::Key::F10, egui::Key::F11, egui::Key::F12,
];
const ARROWS: [egui::Key; 4] = [
    egui::Key::ArrowUp,
    egui::Key::ArrowDown,
    egui::Key::ArrowLeft,
    egui::Key::ArrowRight,
];

fn main() -> eframe::Result {
    // Instância secundária (outra cópia já rodando): só alterna a janela via socket.
    if secondary_instance() {
        return Ok(());
    }
    // Primária: garante estar num escopo systemd "app-*" para o portal
    // GlobalShortcuts conseguir resolver o app-id (exigência do xdg-desktop-portal).
    ensure_app_scope();
    if !bind_primary_instance() {
        return Ok(());
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([WINDOW_W, WINDOW_H])
        .with_min_inner_size([WINDOW_W, WINDOW_H])
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_visible(false);
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "shortcut",
        options,
        Box::new(|cc| Ok(Box::new(ShortcutApp::new(cc)))),
    )
}

fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Mostra a janela no Wayland via script do KWin: desminimiza e ativa.
/// (Sem centralização: o script KWin não tem Qt.rect; a posição é mantida.)
fn kwin_show() {
    kwin_script(r#"
var clients = workspace.windowList();
for (var i = 0; i < clients.length; i++) {
    var c = clients[i];
    if (c.resourceClass === "shortcut") {
        c.minimized = false;
        c.skipTaskbar = true;
        c.skipSwitcher = true;
        c.skipPager = true;
        workspace.activeWindow = c;
        break;
    }
}
"#);
}

/// Executa um script JS no KWin (org.kde.KWin /Scripting).
fn kwin_script(js: &str) {
    // Caminho único por chamada: o KWin deduplica scripts pelo caminho e não
    // recarrega o mesmo arquivo (o script antigo continuaria em efeito).
    let path = format!(
        "/tmp/shortcut-kwin-{}.js",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    if std::fs::write(&path, js).is_err() {
        eprintln!("kwin: falha ao escrever script");
        return;
    }
    eprintln!("kwin: new_session...");
    match dbus::blocking::Connection::new_session() {
        Ok(conn) => {
            eprintln!("kwin: new_session ok");
            let proxy = conn.with_proxy("org.kde.KWin", "/Scripting", Duration::from_secs(3));
            eprintln!("kwin: loadScript chamando...");
            if let Ok((id,)) = proxy.method_call::<(i32,), _, _, _>(
                "org.kde.kwin.Scripting",
                "loadScript",
                (path.as_str(),),
            ) {
                eprintln!("kwin: loadScript id={id}");
            } else {
                eprintln!("kwin: loadScript falhou");
            }
            eprintln!("kwin: start chamando...");
            if let Err(e) =
                proxy.method_call::<(), (), _, _>("org.kde.kwin.Scripting", "start", ())
            {
                eprintln!("kwin: start: {e}");
            }
            eprintln!("kwin: pronto");
        }
        Err(e) => eprintln!("kwin: dbus: {e}"),
    }
}

fn load_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/icon.png");
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(egui::IconData {
        rgba: rgba.to_vec(),
        width,
        height,
        ..Default::default()
    })
}

/// Tenta conectar no socket de uma instância já rodando; se existir,
/// envia "toggle" e retorna `true` (a instância secundária deve sair).
fn secondary_instance() -> bool {
    use std::io::Write;
    let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    let path = dir.join("shortcut.sock");
    if let Ok(mut s) = UnixStream::connect(&path) {
        let _ = s.write_all(b"toggle");
        return true;
    }
    false
}

/// O xdg-desktop-portal só resolve o app-id de processos em units systemd
/// `app-<AppID>-<random>.scope`. Lançado de terminal/script, o processo não tem
/// isso — então nos re-executamos via `systemd-run --user --scope` e o processo
/// original sai. O binário em si roda dentro do escopo e o portal passa a
/// conhecer o app-id "shortcut" (exige shortcut.desktop instalado).
fn ensure_app_scope() {
    if std::env::var_os("SHORTCUT_SCOPED").is_some() {
        return;
    }
    if in_app_scope() {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return,
    };
    let unit = format!("app-shortcut-{}", std::process::id());
    let spawned = std::process::Command::new("systemd-run")
        .env("SHORTCUT_SCOPED", "1")
        .args(["--user", "--scope", "--unit"])
        .arg(&unit)
        .arg(&exe)
        .spawn()
        .is_ok();
    if spawned {
        std::process::exit(0); // o filho continua dentro do escopo
    }
    eprintln!(
        "shortcut: systemd-run indisponível — o atalho global do portal não vai \
         funcionar (use um atalho personalizado do KDE apontando para o binário)."
    );
}

fn in_app_scope() -> bool {
    // Só considera válido se estivermos no NOSSO escopo (app-shortcut-*),
    // senão o portal derivaria o app-id de outro app (ex: o escopo do terminal).
    std::fs::read_to_string("/proc/self/cgroup")
        .map(|c| c.lines().any(|l| l.contains("/app-shortcut-")))
        .unwrap_or(false)
}

/// Single instance via Unix socket: um segundo launch apenas alterna a janela
/// da instância primária (mesmo comportamento do plugin do Tauri).
fn bind_primary_instance() -> bool {
    use std::io::{Read, Write};
    let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    let path = dir.join("shortcut.sock");
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(_) => {
            if let Ok(mut s) = UnixStream::connect(&path) {
                let _ = s.write_all(b"toggle");
                return false;
            }
            let _ = std::fs::remove_file(&path);
            match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(_) => {
                    eprintln!("shortcut: não foi possível criar o socket {path:?}");
                    return true; // segue sem single instance
                }
            }
        }
    };
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut s) = conn else { continue };
            let mut buf = [0u8; 16];
            let _ = s.read(&mut buf);
            TOGGLE.store(true, Ordering::Relaxed);
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
        }
    });
    true
}

#[derive(PartialEq)]
enum Mode {
    Search,
    Settings,
}

struct ShortcutApp {
    query: String,
    last_query: String,
    results: Vec<SearchResult>,
    selected: usize,
    can_paste: bool,
    visible: bool,
    started: bool,
    focus_requested: bool,
    had_focus: bool,
    suppress_until: Option<Instant>,
    last_selected_rect: Option<Rect>,
    mode: Mode,
    shortcut: Option<globalshortcut::GlobalShortcut>,
    capture: bool,
    shortcut_display: String,
    hotkey_enabled: bool,
    max_history: usize,
    save_images: bool,
}

impl ShortcutApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let _ = CTX.set(cc.egui_ctx.clone());
        setup_fonts(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        cc.egui_ctx.style_mut_of(egui::Theme::Dark, |s| {
            s.visuals.widgets.noninteractive.bg_fill = PANEL;
            s.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, DIM);
            s.visuals.widgets.inactive.bg_fill = PANEL.gamma_multiply(1.08);
            s.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
            s.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
            s.visuals.widgets.hovered.bg_fill = SELECT;
            s.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
            s.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
            s.visuals.widgets.active.bg_fill = SELECT;
            s.visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
            s.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
            s.visuals.selection.bg_fill = ACCENT;
            // Sem blink de cursor: ele força repaints contínuos (60fps)
            // e mantém o loop do eframe acordado (CPU alta em idle).
            s.visuals.text_cursor.blink = false;
        });
        let ctx = cc.egui_ctx.clone();
        clipboard::start_watcher(move || {
            CLIP_DIRTY.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        });
        let (hotkey_enabled, default_trigger, max_history, save_images) = {
            let c = config::CONFIG.lock();
            (
                c.hotkey_enabled,
                c.shortcut.clone().unwrap_or_else(|| "Alt+Space".into()),
                c.max_history,
                c.save_images,
            )
        };
        let shortcut = match globalshortcut::GlobalShortcut::register(&default_trigger) {
            Ok(gs) => Some(gs),
            Err(e) => {
                eprintln!(
                    "shortcut: atalho global indisponível ({e}). \
                     Instale shortcut.desktop (veja README) ou use um atalho \
                     personalizado do KDE apontando para o binário."
                );
                None
            }
        };
        let shortcut_display = default_trigger;
        cc.egui_ctx.request_repaint();
        Self {
            query: String::new(),
            last_query: String::new(),
            results: Vec::new(),
            selected: 0,
            can_paste: clipboard::which("wtype").is_some(),
            visible: false,
            started: false,
            focus_requested: false,
            had_focus: false,
            suppress_until: None,
            last_selected_rect: None,
            mode: Mode::Search,
            shortcut,
            capture: false,
            shortcut_display,
            hotkey_enabled,
            max_history,
            save_images,
        }
    }

    fn toggle(&mut self, ctx: &egui::Context) {
        if self.visible {
            self.hide(ctx);
        } else {
            self.show(ctx);
        }
    }

    fn show(&mut self, ctx: &egui::Context) {
        self.visible = true;
        self.mode = Mode::Search;
        self.capture = false;
        self.query.clear();
        self.last_query.clear();
        self.selected = 0;
        self.last_selected_rect = None;
        self.do_search();
        self.focus_requested = true;
        self.had_focus = false;
        self.suppress_until = Some(Instant::now() + Duration::from_millis(350));
        if is_wayland() {
            // No Wayland, winit não consegue mostrar/focar/posicionar; o script do
            // KWin desminimiza, centraliza e ativa a janela.
            kwin_show();
        } else {
            if let Some(ms) = ctx.input(|i| i.viewport().monitor_size) {
                let pos =
                    Pos2::new((ms.x - WINDOW_W) / 2.0, (ms.y - WINDOW_H) / 2.0).max(Pos2::ZERO);
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
            }
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.visible = false;
        if is_wayland() {
            // winit 0.30: set_visible é no-op no Wayland. Minimizamos via script
            // do KWin (e não via ViewportCommand::Minimized) para o loop do
            // eframe continuar rodando e o atalho global seguir funcionando.
            kwin_script(r#"
var clients = workspace.windowList();
for (var i = 0; i < clients.length; i++) {
    var c = clients[i];
    if (c.resourceClass === "shortcut") { c.minimized = true; break; }
}
"#);
        } else {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }
    }

    fn do_search(&mut self) {
        let q = self.query.trim().to_string();
        self.last_query = q.clone();
        let matcher = SkimMatcherV2::default();
        let mut out: Vec<SearchResult> = Vec::new();

        if let Some(value) = search::calc_result(&q) {
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
            // nenhuma busca: mostrar apenas histórico
        } else if matches_settings(&q) {
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
                .filter_map(|a| matcher.fuzzy_match(&a.name, &q).map(|s| (s, a)))
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
                .filter_map(|e| matcher.fuzzy_match(&e.text, &q).map(|s| (s, e)))
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

        self.results = out;
        self.selected = 0;
        self.last_selected_rect = None;
    }

    fn execute(&mut self, ctx: &egui::Context, item: SearchResult) {
        let mut paste_after = false;
        match item.kind.as_str() {
            "app" => {
                if let Err(e) = apps::launch(&item.data) {
                    eprintln!("shortcut: {e}");
                }
                self.hide(ctx);
                return;
            }
            "settings" => {
                self.mode = Mode::Settings;
                self.capture = false;
                ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("search_input")));
                return;
            }
            "text" | "file" => {
                clipboard::set_clipboard(&item.data);
                paste_after = true;
            }
            "image" => {
                clipboard::paste_image(&item.data);
                paste_after = true;
            }
            "calc" => clipboard::set_clipboard(&item.data),
            _ => {}
        }
        self.hide(ctx);
        if paste_after {
            clipboard::maybe_paste();
        }
    }

    fn draw(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let (enter, up, down, esc) = (
            ctx.input(|i| i.key_pressed(egui::Key::Enter)),
            ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)),
            ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)),
            ctx.input(|i| i.key_pressed(egui::Key::Escape)),
        );

        let mut clicked: Option<usize> = None;
        let size = ui.available_size();
        let painter = ui.painter().clone();
        let window = Rect::from_min_size(Pos2::ZERO, size);
        painter.rect_filled(window, RADIUS, PANEL);
        painter.rect_stroke(
            window.shrink(0.5),
            RADIUS,
            Stroke::new(1.0, BORDER),
            egui::StrokeKind::Outside,
        );

        // header (campo de busca)
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let (ir, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
            draw_magnifier(ui.painter(), ir.center(), 8.5);
            ui.add_space(8.0);
            let te = ui.add_sized(
                [ui.available_width(), 26.0],
                TextEdit::singleline(&mut self.query)
                    .id(egui::Id::new("search_input"))
                    .hint_text(RichText::new("Buscar apps, clipboard ou calcular...").color(DIM))
                    .frame(egui::Frame::NONE)
                    .font(FontId::proportional(15.0))
                    .text_color(TEXT),
            );
            if self.focus_requested {
                te.request_focus();
                self.focus_requested = false;
            }
        });
        let div_y = ui.cursor().top() + 6.0;
        ui.painter().line_segment(
            [Pos2::new(14.0, div_y), Pos2::new(size.x - 14.0, div_y)],
            Stroke::new(1.0, BORDER),
        );
        ui.add_space(4.0);

        if self.mode == Mode::Settings {
            self.draw_settings(ui, &ctx, div_y, size, esc);
            painter.text(
                Pos2::new(size.x - 16.0, size.y - FOOTER_H / 2.0),
                Align2::RIGHT_CENTER,
                "Esc voltar",
                FontId::proportional(11.0),
                DIM,
            );
            return;
        }

        // lista
        let list_h = (size.y - div_y - FOOTER_H - 4.0).max(40.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(list_h)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.set_width(size.x);
                if let Some(r) = self.last_selected_rect {
                    ui.scroll_to_rect(r, Some(egui::Align::Center));
                }
                let mut prev_kind: Option<&str> = None;
                for (i, item) in self.results.iter().enumerate() {
                    let kind = item.kind.as_str();
                    if prev_kind != Some(kind) {
                        let label = kind_label(kind);
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            ui.label(RichText::new(label).size(10.5).strong().color(DIM));
                        });
                        ui.add_space(3.0);
                        prev_kind = Some(kind);
                    }
                    let sel = i == self.selected;
                    let resp = draw_row(ui, &ctx, item, sel, self.can_paste);
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                    if sel {
                        self.last_selected_rect = Some(resp.rect);
                    }
                }
                if self.results.is_empty() {
                    ui.add_space(24.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(RichText::new("Nenhum resultado").size(13.0).color(DIM));
                    });
                }
            });

        // footer
        let fy = size.y - FOOTER_H / 2.0;
        let hint = format!(
            "↑↓ navegar · Enter {} · Esc fechar",
            self.results
                .get(self.selected)
                .map(|it| action_label(self.can_paste, it))
                .unwrap_or_default()
        );
        painter.text(
            Pos2::new(16.0, fy),
            Align2::LEFT_CENTER,
            hint,
            FontId::proportional(11.0),
            DIM,
        );
        let count = format!(
            "{} resultado{}",
            self.results.len(),
            if self.results.len() == 1 { "" } else { "s" }
        );
        painter.text(
            Pos2::new(size.x - 16.0, fy),
            Align2::RIGHT_CENTER,
            count,
            FontId::proportional(11.0),
            DIM,
        );

        // ações de teclado do modo busca
        if up && !self.results.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
        if down && !self.results.is_empty() {
            self.selected = (self.selected + 1).min(self.results.len() - 1);
        }
        if esc {
            self.hide(&ctx);
        }
        if let Some(i) = clicked {
            let item = self.results[i].clone();
            self.execute(&ctx, item);
        } else if enter && !self.results.is_empty() {
            let item = self.results[self.selected].clone();
            self.execute(&ctx, item);
        }
    }

    fn draw_settings(&mut self, ui: &mut Ui, ctx: &egui::Context, div_y: f32, size: Vec2, _esc: bool) {
        // captura de novo atalho (antes dos widgets, para não perder os eventos)
        if self.capture {
            if let Some(combo) = capture_combo(ctx) {
                if let Some(gs) = &self.shortcut {
                    let _ = gs.set_trigger(&combo);
                }
                {
                    let mut cfg = config::CONFIG.lock();
                    cfg.shortcut = Some(combo.clone());
                    cfg.save();
                }
                self.shortcut_display = combo;
                self.capture = false;
            } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.capture = false;
            }
        }
        // mostra o trigger aceito pelo portal
        if let Some(gs) = &self.shortcut {
            if let Some(t) = gs.current_trigger() {
                self.shortcut_display = t;
            }
        }

        let list_h = (size.y - div_y - FOOTER_H - 4.0).max(40.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(list_h)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.set_width(size.x);

                ui.add_space(8.0);
                settings_title(ui, "Atalho global");
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(RichText::new("Combinação").color(DIM).size(12.5));
                    ui.add_space(10.0);
                    let text = if self.capture {
                        "Pressione a combinação… (Esc cancela)"
                    } else if !self.hotkey_enabled {
                        "desabilitado"
                    } else {
                        self.shortcut_display.as_str()
                    };
                    let boxed = egui::Button::new(RichText::new(text).color(TEXT).size(12.5))
                        .fill(if self.capture {
                            ACCENT.gamma_multiply(0.25)
                        } else {
                            PANEL.gamma_multiply(1.16)
                        })
                        .stroke(Stroke::new(1.0, BORDER))
                        .corner_radius(6.0)
                        .min_size(Vec2::new(220.0, 28.0));
                    if ui.add(boxed).clicked() && self.hotkey_enabled && !self.capture {
                        self.capture = true;
                    }
                    ui.add_space(8.0);
                    let change = egui::Button::new("Alterar")
                        .corner_radius(6.0)
                        .min_size(Vec2::new(74.0, 28.0));
                    if ui.add(change).clicked() && self.hotkey_enabled && !self.capture {
                        self.capture = true;
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let mut on = self.hotkey_enabled;
                    if ui.checkbox(&mut on, "Atalho global habilitado").changed() {
                        self.hotkey_enabled = on;
                        self.persist_config();
                    }
                });

                ui.add_space(14.0);
                settings_title(ui, "Histórico da área de transferência");
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(RichText::new("Máx. itens salvos").color(DIM).size(12.5));
                    ui.add_space(10.0);
                    let mut v = self.max_history;
                    if stepper(ui, &mut v, 50, 10, 1000) {
                        self.max_history = v;
                        self.persist_config();
                        clipboard::HISTORY.lock().apply_limit();
                    }
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let mut on = self.save_images;
                    if ui.checkbox(&mut on, "Salvar imagens copiadas").changed() {
                        self.save_images = on;
                        self.persist_config();
                    }
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let n = {
                        let h = clipboard::HISTORY.lock();
                        format!(
                            "Armazenado: {} itens ({})",
                            h.entries.len(),
                            h.count_images()
                        )
                    };
                    ui.label(RichText::new(n).color(DIM).size(12.0));
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let clear = egui::Button::new("Limpar histórico")
                        .fill(ACCENT.gamma_multiply(0.18))
                        .corner_radius(6.0)
                        .min_size(Vec2::new(140.0, 28.0));
                    if ui.add(clear).clicked() {
                        clipboard::HISTORY.lock().clear();
                    }
                });

                ui.add_space(14.0);
                settings_title(ui, "Arquivos");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(
                            "Arquivos copiados (ex: no Dolphin) são detectados \
                             automaticamente como uri-list.",
                        )
                        .color(DIM)
                        .size(12.0),
                    );
                });
                ui.add_space(6.0);
                if self.shortcut.is_none() {
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new(
                                "O atalho global não está disponível: instale \
                                 shortcut.desktop ou configure um atalho do KDE \
                                 apontando para o binário.",
                            )
                            .color(Color32::from_rgb(0xec, 0x9a, 0x9a))
                            .size(12.0),
                        );
                    });
                }
                ui.add_space(10.0);
            });
    }

    fn persist_config(&self) {
        let mut cfg = config::CONFIG.lock();
        cfg.max_history = self.max_history;
        cfg.save_images = self.save_images;
        cfg.hotkey_enabled = self.hotkey_enabled;
        cfg.save();
    }
}

impl eframe::App for ShortcutApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.started {
            self.started = true;
            self.show(ctx);
        }
        if TOGGLE.swap(false, Ordering::Relaxed)
            || (globalshortcut::SHORTCUT_PRESSED.swap(false, Ordering::SeqCst)
                && self.hotkey_enabled)
        {
            eprintln!("dbg: toggle (visivel={})", self.visible);
            self.toggle(ctx);
        }
        if CLIP_DIRTY.swap(false, Ordering::Relaxed) && self.visible {
            self.do_search();
        }
        if self.visible {
            if self.query != self.last_query {
                if self.mode == Mode::Settings {
                    self.mode = Mode::Search;
                }
                self.do_search();
            }
            let suppress = self
                .suppress_until
                .map(|t| Instant::now() < t)
                .unwrap_or(false);
            let focused = ctx.input(|i| i.focused);
            if focused {
                self.had_focus = true;
            }
            // Só esconde por blur se a janela já teve foco (se o compositor
            // nunca conceder foco, ela fica visível até Esc/Enter).
            // SHORTCUT_KEEP=1 desativa o blur-hide (útil para debug/inspeção)
            if !suppress
                && self.had_focus
                && !focused
                && std::env::var_os("SHORTCUT_KEEP").is_none()
            {
                self.hide(ctx);
            }
        }
        // Oculto: mantém wake periódico para o flag do atalho global chegar.
        // Visível: o loop dorme entre eventos (o portal acorda pontualmente).
        if !self.visible {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.visible {
            self.draw(ui);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Totalmente transparente: os cantos arredondados deixam o desktop aparecer
        [0.0, 0.0, 0.0, 0.0]
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for p in [
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(p) {
            let name = "dejavu".to_string();
            fonts
                .font_data
                .insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
            for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push(name.clone());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
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
            Path::new(p.trim_end_matches('/'))
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

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "calc" => "Calculadora",
        "app" => "Aplicativos",
        "settings" => "Configurações",
        "text" => "Área de transferência",
        "image" => "Imagens",
        "file" => "Arquivos",
        _ => "Outros",
    }
}

fn action_label(can_paste: bool, item: &SearchResult) -> String {
    match item.kind.as_str() {
        "app" => "Abrir".into(),
        "calc" => "Copiar resultado".into(),
        "settings" => "Abrir configurações".into(),
        _ => {
            if can_paste {
                "Colar".into()
            } else {
                "Copiar".into()
            }
        }
    }
}

fn icon_tex_id(ctx: &egui::Context, path: &str) -> Option<TextureId> {
    egui::Image::from_uri(format!("file://{path}"))
        .load_for_size(ctx, Vec2::splat(26.0))
        .ok()?
        .texture_id()
}

fn draw_row(
    ui: &mut Ui,
    ctx: &egui::Context,
    item: &SearchResult,
    selected: bool,
    can_paste: bool,
) -> Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 8.0, SELECT);
    } else if resp.hovered() {
        painter.rect_filled(rect, 8.0, HOVER);
    }

    let icon_size = 26.0;
    let ic = Rect::from_center_size(
        Pos2::new(rect.min.x + 12.0 + icon_size / 2.0, rect.center().y),
        Vec2::splat(icon_size),
    );
    let tex = if matches!(item.kind.as_str(), "app" | "image") {
        item.icon.as_deref().and_then(|p| icon_tex_id(ctx, p))
    } else {
        None
    };
    match tex {
        Some(id) => {
            painter.image(
                id,
                ic,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => match item.kind.as_str() {
            "calc" => {
                painter.rect_filled(ic, 7.0, ACCENT.gamma_multiply(0.25));
                painter.text(
                    ic.center(),
                    Align2::CENTER_CENTER,
                    "=",
                    FontId::proportional(14.0),
                    ACCENT,
                );
            }
            "text" => draw_clipboard(painter, ic),
            "file" => draw_file(painter, ic),
            "settings" => draw_sliders(painter, ic),
            _ => {}
        },
    }

    let tx = rect.min.x + 12.0 + icon_size + 10.0;
    painter.text(
        Pos2::new(tx, rect.min.y + 5.0),
        Align2::LEFT_TOP,
        &item.title,
        FontId::proportional(14.0),
        TEXT,
    );
    painter.text(
        Pos2::new(tx, rect.min.y + 22.0),
        Align2::LEFT_TOP,
        &item.subtitle,
        FontId::proportional(11.0),
        DIM,
    );
    if selected {
        let act = action_label(can_paste, item);
        painter.text(
            Pos2::new(rect.right() - 12.0, rect.center().y),
            Align2::RIGHT_CENTER,
            act,
            FontId::proportional(11.0),
            DIM,
        );
    }
    resp
}

fn settings_title(ui: &mut Ui, title: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(RichText::new(title).size(11.0).strong().color(DIM));
    });
}

fn stepper(ui: &mut Ui, value: &mut usize, step: usize, min: usize, max: usize) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let minus = egui::Button::new(RichText::new("-").size(16.0))
            .corner_radius(6.0)
            .min_size(Vec2::splat(26.0));
        if ui.add(minus).clicked() {
            *value = value.saturating_sub(step).max(min);
            changed = true;
        }
        ui.add_space(6.0);
        ui.label(RichText::new(value.to_string()).size(13.0).color(TEXT));
        ui.add_space(6.0);
        let plus = egui::Button::new(RichText::new("+").size(16.0))
            .corner_radius(6.0)
            .min_size(Vec2::splat(26.0));
        if ui.add(plus).clicked() {
            *value = (*value + step).min(max);
            changed = true;
        }
    });
    changed
}

/// Lê a combinação pressionada agora (modificadores + tecla) no formato Qt,
/// ex: "Alt+Space", "Ctrl+Shift+F2". Exige ao menos um modificador, exceto
/// F-keys.
fn capture_combo(ctx: &egui::Context) -> Option<String> {
    let mods = ctx.input(|i| i.modifiers);
    let key = ctx.input(|i| {
        for k in LETTERS.iter().chain(DIGITS.iter()).chain(FN_KEYS.iter()).chain(ARROWS.iter())
        {
            if i.key_pressed(*k) {
                return Some(*k);
            }
        }
        if i.key_pressed(egui::Key::Space) {
            return Some(egui::Key::Space);
        }
        None
    })?;
    let mut parts: Vec<String> = Vec::new();
    if mods.command {
        parts.push("Meta".into());
    }
    if mods.ctrl {
        parts.push("Ctrl".into());
    }
    if mods.alt {
        parts.push("Alt".into());
    }
    if mods.shift {
        parts.push("Shift".into());
    }
    if parts.is_empty() && !(FN_KEYS.contains(&key) || ARROWS.contains(&key) || key == egui::Key::Space) {
        return None;
    }
    parts.push(key_name(key));
    Some(parts.join("+"))
}

fn key_name(k: egui::Key) -> String {
    let s = format!("{k:?}");
    if let Some(d) = s.strip_prefix("Num") {
        d.to_string()
    } else if let Some(d) = s.strip_prefix("Arrow") {
        d.to_string()
    } else if s == "Space" {
        "Space".into()
    } else {
        s
    }
}

fn draw_magnifier(painter: &egui::Painter, center: Pos2, r: f32) {
    painter.circle_stroke(center, r, Stroke::new(1.8, DIM));
    let a = std::f32::consts::FRAC_PI_4;
    let start = center + Vec2::new(r * 0.7 * a.cos(), r * 0.7 * a.sin());
    let end = center + Vec2::new((r + 3.5) * a.cos(), (r + 3.5) * a.sin());
    painter.line_segment([start, end], Stroke::new(1.8, DIM));
}

fn draw_clipboard(painter: &egui::Painter, rect: Rect) {
    let tab = Rect::from_center_size(
        Pos2::new(rect.center().x, rect.min.y + 3.5),
        Vec2::new(9.0, 5.0),
    );
    painter.rect_filled(tab, 1.5, DIM);
    let body = Rect::from_min_max(
        Pos2::new(rect.min.x + 3.0, rect.min.y + 6.5),
        Pos2::new(rect.max.x - 3.0, rect.max.y - 1.0),
    );
    painter.rect_stroke(body, 2.5, Stroke::new(1.6, DIM), egui::StrokeKind::Inside);
    painter.line_segment(
        [
            Pos2::new(body.min.x + 5.0, body.min.y + 4.5),
            Pos2::new(body.max.x - 5.0, body.min.y + 4.5),
        ],
        Stroke::new(1.6, DIM),
    );
}

fn draw_file(painter: &egui::Painter, rect: Rect) {
    let body = Rect::from_min_max(
        Pos2::new(rect.min.x + 3.0, rect.min.y + 2.5),
        Pos2::new(rect.max.x - 3.0, rect.max.y - 2.0),
    );
    painter.rect_stroke(body, 2.0, Stroke::new(1.6, DIM), egui::StrokeKind::Inside);
    painter.line_segment(
        [
            Pos2::new(body.min.x + 2.0, body.min.y + 5.0),
            Pos2::new(body.max.x - 2.0, body.min.y + 5.0),
        ],
        Stroke::new(1.6, DIM),
    );
    painter.line_segment(
        [
            Pos2::new(body.min.x + 2.0, body.max.y - 5.0),
            Pos2::new(body.max.x - 6.0, body.max.y - 5.0),
        ],
        Stroke::new(1.6, DIM),
    );
}

fn draw_sliders(painter: &egui::Painter, rect: Rect) {
    for (i, dx) in [6.0, 0.0, 3.0].into_iter().enumerate() {
        let y = rect.min.y + 7.0 + i as f32 * 7.0;
        painter.line_segment(
            [
                Pos2::new(rect.min.x + 2.0, y),
                Pos2::new(rect.max.x - 2.0, y),
            ],
            Stroke::new(2.0, DIM),
        );
        painter.circle_filled(Pos2::new(rect.min.x + 8.0 + dx, y), 3.0, DIM);
    }
}