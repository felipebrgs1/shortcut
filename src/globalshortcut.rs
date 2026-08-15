//! Atalho global via portal `org.freedesktop.portal.GlobalShortcuts`.
//!
//! O portal (xdg-desktop-portal-kde) registra o atalho no kglobalaccel e emite
//! `Activated` (unicast) para a conexão dona da sessão. Uma thread dedicada é a
//! dona exclusiva da conexão (sinais + method calls), então o main thread não
//! bloqueia em D-Bus nem há corrida de mensagens.
//!
//! Requer app-id resolvível (processo em unit systemd `app-*`, garantido pelo
//! re-exec via `systemd-run --user --scope`) e shortcut.desktop instalado.

use dbus::arg::Variant;
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const PORTAL_SVC: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const SHORTCUT_ID: &str = "toggle";
const TIMEOUT: Duration = Duration::from_secs(3);

/// Sinalizado quando o atalho é pressionado.
pub static SHORTCUT_PRESSED: AtomicBool = AtomicBool::new(false);
pub static PRESS_COUNT: AtomicUsize = AtomicUsize::new(0);

enum Cmd {
    SetTrigger(String, mpsc::SyncSender<Result<(), String>>),
}

pub struct GlobalShortcut {
    tx: mpsc::Sender<Cmd>,
    trigger: Arc<Mutex<Option<String>>>,
}

impl GlobalShortcut {
    /// Registra o atalho global com o trigger preferido (ex: "Alt+Space").
    pub fn register(preferred: &str) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let trigger = Arc::new(Mutex::new(Some(preferred.to_string())));
        let t2 = Arc::clone(&trigger);
        let preferred = preferred.to_string();
        std::thread::spawn(move || portal_thread(rx, ready_tx, preferred, t2));
        ready_rx.recv().map_err(|e| e.to_string())??;
        Ok(Self { tx, trigger })
    }

    /// Troca o trigger (ex: "Ctrl+Shift+Space") no portal/kglobalaccel.
    pub fn set_trigger(&self, trigger: &str) -> Result<(), String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.tx
            .send(Cmd::SetTrigger(trigger.to_string(), reply_tx))
            .map_err(|e| e.to_string())?;
        reply_rx.recv().map_err(|e| e.to_string())?
    }

    /// Trigger aceito mais recentemente (atualizado pelo ShortcutsChanged).
    pub fn current_trigger(&self) -> Option<String> {
        self.trigger.lock().unwrap().clone()
    }
}

fn portal_thread(
    rx: mpsc::Receiver<Cmd>,
    ready_tx: mpsc::SyncSender<Result<(), String>>,
    preferred: String,
    trigger: Arc<Mutex<Option<String>>>,
) {
    let conn = match Connection::new_session() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready_tx.send(Err(e.to_string()));
            return;
        }
    };

    // CreateSession(a{sv}) -> o. O token de sessão é OBRIGATÓRIO: sem ele o
    // xdg-desktop-portal desta versão dá assert e aborta.
    let token = format!("shortcut_{}", std::process::id());
    let mut session_opts = HashMap::new();
    session_opts.insert("session_handle_token".to_string(), Variant(token.clone()));
    let res: Result<(dbus::Path<'static>,), _> = conn
        .with_proxy(PORTAL_SVC, PORTAL_PATH, TIMEOUT)
        .method_call(PORTAL_IFACE, "CreateSession", (session_opts,));
    if let Err(e) = res {
        let _ = ready_tx.send(Err(format!("CreateSession: {e}")));
        return;
    }

    // O caminho da sessão é derivado pelo core: /desktop/session/<sender>/<token>
    let sender = conn
        .unique_name()
        .trim_start_matches(':')
        .replace('.', "_");
    let session = format!("/org/freedesktop/portal/desktop/session/{sender}/{token}");

    // Bind inicial com retry (a sessão é criada assincronamente no portal).
    if let Err(e) = bind_trigger(&conn, &session, &preferred) {
        let _ = ready_tx.send(Err(e));
        return;
    }
    *trigger.lock().unwrap() = Some(preferred.clone());
    let _ = ready_tx.send(Ok(()));

    watch_activated(&conn, session.clone());
    watch_changed(&conn, session.clone(), Arc::clone(&trigger));

    // Loop: processa sinais e comandos. Dona exclusiva da conexão.
    loop {
        let _ = conn.process(Duration::from_millis(50));
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::SetTrigger(t, reply) => {
                    let res = bind_trigger(&conn, &session, &t);
                    if res.is_ok() {
                        *trigger.lock().unwrap() = Some(t);
                    }
                    let _ = reply.send(res);
                }
            }
        }
    }
}

fn bind_trigger(conn: &Connection, session: &str, trigger: &str) -> Result<(), String> {
    let mut last_err = String::from("unknown");
    for _ in 0..20 {
        let shortcuts: Vec<(String, HashMap<String, Variant<&str>>)> = vec![(
            SHORTCUT_ID.to_string(),
            HashMap::from([
                ("description".into(), Variant("Alternar launcher")),
                ("preferred_trigger".into(), Variant(trigger)),
            ]),
        )];
        let res: Result<(dbus::Path<'static>,), _> = conn
            .with_proxy(PORTAL_SVC, PORTAL_PATH, TIMEOUT)
            .method_call(
                PORTAL_IFACE,
                "BindShortcuts",
                (
                    dbus::Path::from(session.to_string()),
                    shortcuts,
                    String::new(), // parent_window
                    HashMap::<String, Variant<u32>>::new(), // options
                ),
            );
        match res {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_err = e.to_string();
                if !e.to_string().contains("Invalid session") {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Err(format!("BindShortcuts: {last_err}"))
}

fn watch_activated(conn: &Connection, session: String) {
    let mut rule = MatchRule::new();
    rule.msg_type = Some(dbus::MessageType::Signal);
    let rule = rule
        .with_interface(PORTAL_IFACE)
        .with_member("Activated")
        .with_path(PORTAL_PATH);
    // Assinatura: (o session, s shortcut_id, t timestamp, a{sv} options)
    let _ = conn.add_match(
        rule,
        move |args: (
            dbus::Path,
            String,
            u64,
            HashMap<String, Variant<String>>,
        ), _conn, _msg| {
            if args.0.to_string() == session && args.1 == SHORTCUT_ID {
                PRESS_COUNT.fetch_add(1, Ordering::Relaxed);
                SHORTCUT_PRESSED.store(true, Ordering::SeqCst);
                // Wake pontual (one-shot) para o main thread ver o flag.
                if let Some(ctx) = crate::CTX.get() {
                    ctx.request_repaint();
                }
            }
            true
        },
    );
}

fn watch_changed(conn: &Connection, session: String, trigger: Arc<Mutex<Option<String>>>) {
    let mut rule = MatchRule::new();
    rule.msg_type = Some(dbus::MessageType::Signal);
    let rule = rule
        .with_interface(PORTAL_IFACE)
        .with_member("ShortcutsChanged")
        .with_path(PORTAL_PATH);
    // Assinatura: (o session, a(sa{sv}) shortcuts)
    let _ = conn.add_match(
        rule,
        move |args: (
            dbus::Path,
            Vec<(String, HashMap<String, Variant<String>>)>,
        ), _conn, _msg| {
            if args.0.to_string() == session {
                if let Some((_, opts)) = args.1.into_iter().find(|(id, _)| id == SHORTCUT_ID) {
                    if let Some(t) = opts.get("trigger").map(|v| v.0.clone()) {
                        *trigger.lock().unwrap() = Some(t);
                    }
                }
            }
            true
        },
    );
}