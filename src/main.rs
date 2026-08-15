mod apps;
mod clipboard;
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
static CTX: OnceLock<egui::Context> = OnceLock::new();

fn main() -> eframe::Result {
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
}

impl ShortcutApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let _ = CTX.set(cc.egui_ctx.clone());
        setup_fonts(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let ctx = cc.egui_ctx.clone();
        clipboard::start_watcher(move || {
            CLIP_DIRTY.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        });
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
        self.query.clear();
        self.last_query.clear();
        self.selected = 0;
        self.last_selected_rect = None;
        self.do_search();
        self.focus_requested = true;
        self.had_focus = false;
        self.suppress_until = Some(Instant::now() + Duration::from_millis(350));
        if let Some(ms) = ctx.input(|i| i.viewport().monitor_size) {
            let pos = Pos2::new((ms.x - WINDOW_W) / 2.0, (ms.y - WINDOW_H) / 2.0).max(Pos2::ZERO);
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        }
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.visible = false;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
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
            out.push(SearchResult {
                kind: "clipboard".into(),
                title: search::one_line(&entry.text),
                subtitle: search::rel_time(entry.ts),
                icon: None,
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
            }
            "clipboard" => {
                clipboard::set_clipboard(&item.data);
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

        // header
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let (ir, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
            draw_magnifier(ui.painter(), ir.center(), 8.5);
            ui.add_space(8.0);
            let te = ui.add_sized(
                [ui.available_width(), 26.0],
                TextEdit::singleline(&mut self.query)
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

        // ações de teclado
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
}

impl eframe::App for ShortcutApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.started {
            self.started = true;
            self.show(ctx);
        }
        if TOGGLE.swap(false, Ordering::Relaxed) {
            self.toggle(ctx);
        }
        if CLIP_DIRTY.swap(false, Ordering::Relaxed) && self.visible {
            self.do_search();
        }
        if self.visible {
            if self.query != self.last_query {
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

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "calc" => "Calculadora",
        "app" => "Aplicativos",
        "clipboard" => "Área de transferência",
        _ => "Outros",
    }
}

fn action_label(can_paste: bool, item: &SearchResult) -> String {
    match item.kind.as_str() {
        "app" => "Abrir".into(),
        "calc" => "Copiar resultado".into(),
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
    let tex = if item.kind == "app" {
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
            "clipboard" => draw_clipboard(painter, ic),
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
