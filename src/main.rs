mod audio;
mod config;
mod engine;
mod ibus;
mod models;
mod portal;
mod text;
mod tray;
mod ui;

use adw::prelude::*;
use config::{Config, APP_ID};
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Longest a recording may run, in case the shortcut release is lost.
const MAX_RECORDING_SECS: u32 = 120;

#[derive(Debug, Clone)]
pub enum Cmd {
    /// Shortcut pressed: start capturing.
    KeyDown,
    /// Shortcut released: transcribe, then type.
    KeyUp,
    /// Guard armed at the start of the numbered capture, ignored if another
    /// capture has started since.
    RecordingTimeout(u64),
    /// The key the compositor actually bound, as it should be displayed.
    ShortcutBound(String),
    OpenPreferences,
    Quit,
}

pub struct State {
    pub cfg: RefCell<Config>,
    pub tx: async_channel::Sender<Cmd>,
    /// What the portal reports as the current trigger, for display only.
    pub shortcut: RefCell<String>,
    /// Row showing the trigger, kept so it can be refreshed when the binding
    /// changes while the settings window is open.
    pub shortcut_row: RefCell<Option<adw::ActionRow>>,
    recorder: RefCell<Option<audio::Recorder>>,
    recording: Cell<u64>,
    /// Registered once, lazily. `None` after a failed attempt: IBus may not be
    /// running at all, and retrying on every dictation would only add latency.
    ibus: RefCell<Option<Rc<ibus::Ibus>>>,
    ibus_tried: Cell<bool>,
    typist: RefCell<Option<Rc<portal::Typist>>>,
}

impl State {
    /// Re-reads the configuration from disk before each dictation, so an edit
    /// made in the file takes effect without a restart. Every setting from the
    /// UI is written immediately, so reloading cannot lose a change.
    pub fn reload(&self) {
        *self.cfg.borrow_mut() = Config::load();
    }

    pub fn update_cfg(&self, f: impl FnOnce(&mut Config)) {
        let mut cfg = self.cfg.borrow_mut();
        f(&mut cfg);
        if let Err(e) = cfg.save() {
            log::error!("saving the configuration: {e}");
        }
    }
}

fn main() -> glib::ExitCode {
    env_logger::init();
    let app = adw::Application::builder().application_id(APP_ID).build();
    let (tx, rx) = async_channel::unbounded();

    app.connect_startup({
        let tx = tx.clone();
        move |app| {
            let cfg = Config::load();
            let state = Rc::new(State {
                shortcut: RefCell::new(cfg.shortcut.clone()),
                cfg: RefCell::new(cfg),
                tx: tx.clone(),
                shortcut_row: RefCell::new(None),
                recorder: RefCell::new(None),
                recording: Cell::new(0),
                ibus: RefCell::new(None),
                ibus_tried: Cell::new(false),
                typist: RefCell::new(None),
            });

            // The application outlives the settings window: the guard is held
            // by the loop below.
            let hold = app.hold();
            tray::spawn(tx.clone());
            spawn_shortcut_listener(&state);
            glib::spawn_future_local({
                let state = state.clone();
                async move {
                    if let Err(e) = request_background().await {
                        log::warn!("Background portal: {e}");
                    }
                    // Registered up front so the first dictation does not pay
                    // for connecting and registering. The RemoteDesktop
                    // session is deliberately not pre-warmed: it is only the
                    // fallback, and asking for keyboard injection rights at
                    // every start would prompt for a permission most sessions
                    // never use.
                    ibus(&state).await;
                }
            });

            let app = app.clone();
            let rx = rx.clone();
            glib::spawn_future_local(async move {
                let _hold = hold;
                while let Ok(cmd) = rx.recv().await {
                    handle(&app, &state, cmd).await;
                }
            });
        }
    });

    // Started by autostart, the application comes up without a window. Any
    // later activation opens the settings.
    let headless_start = Cell::new(std::env::args().any(|a| a == "--background"));
    app.connect_activate(move |_| {
        if headless_start.replace(false) {
            return;
        }
        let _ = tx.try_send(Cmd::OpenPreferences);
    });

    app.run_with_args::<&str>(&[])
}

fn spawn_shortcut_listener(state: &Rc<State>) {
    let trigger = state.cfg.borrow().shortcut.clone();
    let tx = state.tx.clone();
    glib::spawn_future_local(async move {
        if let Err(e) = portal::listen_shortcut(trigger, tx).await {
            log::error!("GlobalShortcuts portal: {e}");
        }
    });
}

async fn handle(app: &adw::Application, state: &Rc<State>, cmd: Cmd) {
    match cmd {
        Cmd::KeyDown => start_recording(app, state),
        Cmd::KeyUp => stop_recording(app, state).await,
        Cmd::RecordingTimeout(id) => {
            if state.recording.get() == id {
                log::warn!("shortcut release never received, capture stopped");
                stop_recording(app, state).await;
            }
        }
        Cmd::ShortcutBound(description) => {
            if let Some(row) = state.shortcut_row.borrow().as_ref() {
                row.set_subtitle(&ui::shortcut_subtitle(&description));
            }
            *state.shortcut.borrow_mut() = description;
        }
        Cmd::OpenPreferences => ui::preferences(app, state),
        Cmd::Quit => app.quit(),
    }
}

fn start_recording(app: &adw::Application, state: &Rc<State>) {
    // The portal repeats Activated while the key is held.
    if state.recorder.borrow().is_some() {
        return;
    }
    // Checked before capturing rather than after: a missing model would
    // otherwise only surface once the user has finished speaking.
    let (engine, model) = {
        let cfg = state.cfg.borrow();
        (cfg.engine, cfg.model.clone())
    };
    if !models::is_installed(engine, &model) {
        let label = models::info(&model).map_or(model.as_str(), |m| m.label);
        notify(
            app,
            &format!("The {label} model is not downloaded. Open Susurre settings to get it."),
        );
        return;
    }
    match audio::Recorder::start() {
        Ok(recorder) => {
            log::info!("recording started");
            *state.recorder.borrow_mut() = Some(recorder);

            let id = state.recording.get().wrapping_add(1);
            state.recording.set(id);
            let tx = state.tx.clone();
            glib::timeout_add_seconds_local_once(MAX_RECORDING_SECS, move || {
                let _ = tx.try_send(Cmd::RecordingTimeout(id));
            });
        }
        Err(e) => {
            log::error!("microphone: {e}");
            notify(app, &format!("Microphone unavailable: {e}"));
        }
    }
}

async fn stop_recording(app: &adw::Application, state: &Rc<State>) {
    let Some(recorder) = state.recorder.borrow_mut().take() else {
        return;
    };
    let pcm = recorder.finish();
    log::info!(
        "recording stopped: {:.1} s",
        pcm.len() as f32 / audio::TARGET_RATE as f32
    );
    // Under a quarter of a second: a stray key press.
    if pcm.len() < audio::TARGET_RATE as usize / 4 {
        return;
    }
    let Some(pcm) = audio::trim_silence(&pcm) else {
        log::info!("no speech detected");
        return;
    };
    state.reload();
    if let Err(e) = dictate(state, pcm).await {
        log::error!("dictation: {e}");
        notify(app, &e.to_string());
    }
}

async fn dictate(state: &Rc<State>, pcm: Vec<f32>) -> anyhow::Result<()> {
    let (engine, model, language, prompt, replacements) = {
        let cfg = state.cfg.borrow();
        (
            cfg.engine,
            cfg.model.clone(),
            cfg.language.clone(),
            cfg.prompt.clone(),
            cfg.replacements.clone(),
        )
    };

    // Transcription blocks for seconds: off the main thread.
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(engine::transcribe(engine, &model, &pcm, &language, &prompt));
    });
    let transcript = rx.recv().await??;
    log::info!("transcript: {transcript:?}");

    let typed = text::apply_replacements(&transcript, &replacements);
    if typed.is_empty() {
        return Ok(());
    }
    deliver(state, &typed).await
}

/// Registers the IBus engine on first use, and remembers a failure so a
/// machine without IBus does not pay for the attempt at every dictation.
async fn ibus(state: &Rc<State>) -> Option<Rc<ibus::Ibus>> {
    if let Some(ibus) = state.ibus.borrow().clone() {
        return Some(ibus);
    }
    if state.ibus_tried.replace(true) {
        return None;
    }
    match ibus::Ibus::connect().await {
        Ok(ibus) => {
            let ibus = Rc::new(ibus);
            *state.ibus.borrow_mut() = Some(ibus.clone());
            Some(ibus)
        }
        Err(e) => {
            log::info!("no IBus engine ({e}); falling back to the portal");
            None
        }
    }
}

/// Inserts the text, preferring the input method.
///
/// IBus commits text instead of simulating keys, so it is not bound by the
/// keyboard layout. The portal is only there for a session without IBus, and
/// it reports what it cannot type rather than dropping it.
async fn deliver(state: &Rc<State>, text: &str) -> anyhow::Result<()> {
    if let Some(ibus) = ibus(state).await {
        match ibus.commit(text).await {
            Ok(()) => return Ok(()),
            Err(e) => log::warn!("IBus commit failed, falling back to the portal: {e}"),
        }
    }
    typist(state).await?.type_text(text).await
}

/// The RemoteDesktop session needs the user's consent, so it is created only
/// when the fallback is actually reached, once, and its restore token is kept.
async fn typist(state: &Rc<State>) -> anyhow::Result<Rc<portal::Typist>> {
    // Borrow released before the first await: borrow_mut() follows below.
    let existing = state.typist.borrow().clone();
    if let Some(typist) = existing {
        return Ok(typist);
    }
    let token = state.cfg.borrow().remote_desktop_token.clone();
    let typist = Rc::new(portal::Typist::new(token).await?);
    if typist.restore_token.is_some() {
        let token = typist.restore_token.clone();
        state.update_cfg(|c| c.remote_desktop_token = token);
    }
    *state.typist.borrow_mut() = Some(typist.clone());
    Ok(typist)
}

/// A dictation that fails without showing anything looks like a broken app.
fn notify(app: &adw::Application, message: &str) {
    let notification = gtk::gio::Notification::new("Dictation failed");
    notification.set_body(Some(message));
    app.send_notification(Some("susurre-error"), &notification);
}

/// Declares the application as a background service (GNOME's "Background
/// Applications" menu, and autostart).
async fn request_background() -> ashpd::Result<()> {
    ashpd::desktop::background::Background::request()
        .reason("Susurre listens for the dictation shortcut in the background")
        .auto_start(true)
        .command(["susurre", "--background"])
        .send()
        .await?
        .response()?;
    Ok(())
}

