use crate::config::{Engine, APP_ID};
use crate::models::ModelInfo;
use crate::{engine, models, text, State};
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

const LANGUAGES: &[(&str, &str)] = &[
    ("auto", "Detect automatically"),
    ("fr", "French"),
    ("en", "English"),
    ("de", "German"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("nl", "Dutch"),
    ("pt", "Portuguese"),
];

/// Callbacks that bring the model rows back in line with the configuration and
/// what is on disk.
type Refreshers = Rc<RefCell<Vec<Box<dyn Fn()>>>>;

fn refresh_all(refreshers: &Refreshers) {
    for refresh in refreshers.borrow().iter() {
        refresh();
    }
}

/// Row subtitles are Pango markup, and the portal describes triggers with
/// angle brackets ("Press <Control><Alt>d"), which would be parsed as tags.
pub fn shortcut_subtitle(trigger: &str) -> String {
    format!("{} — hold to speak", glib::markup_escape_text(trigger))
}

pub fn preferences(app: &adw::Application, state: &Rc<State>) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    let window = adw::PreferencesWindow::builder()
        .application(app)
        .title("Susurre")
        .default_width(600)
        .default_height(760)
        .hide_on_close(true)
        .build();

    let refreshers: Refreshers = Rc::new(RefCell::new(Vec::new()));
    let page = adw::PreferencesPage::new();
    page.add(&general_group(state, &refreshers));
    page.add(&replacements_group(state));
    page.add(&models_group(state, &window, &refreshers));
    window.add(&page);

    refresh_all(&refreshers);
    window.present();
}

fn combo(title: &str, items: &[&str], selected: u32) -> adw::ComboRow {
    adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(items))
        .selected(selected)
        .build()
}

fn general_group(state: &Rc<State>, refreshers: &Refreshers) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("General").build();
    let cfg = state.cfg.borrow().clone();

    let engines: Vec<&str> = Engine::ALL.iter().map(|e| e.label()).collect();
    let selected = Engine::ALL.iter().position(|e| *e == cfg.engine).unwrap_or(0);
    let engine_row = combo("Engine", &engines, selected as u32);
    engine_row.connect_selected_notify({
        let (state, refreshers) = (state.clone(), refreshers.clone());
        move |row| {
            let engine = Engine::ALL[row.selected() as usize];
            state.update_cfg(|c| c.engine = engine);
            engine::unload();
            refresh_all(&refreshers);
        }
    });
    group.add(&engine_row);

    let names: Vec<&str> = models::CATALOG.iter().map(|m| m.label).collect();
    let selected = models::CATALOG.iter().position(|m| m.id == cfg.model).unwrap_or(0);
    let model_row = combo("Model", &names, selected as u32);
    model_row.connect_selected_notify({
        let (state, refreshers) = (state.clone(), refreshers.clone());
        move |row| {
            let model = models::CATALOG[row.selected() as usize].id;
            state.update_cfg(|c| c.model = model.to_string());
            engine::unload();
            refresh_all(&refreshers);
        }
    });
    // The selected model is the one dictation will use: say so when it is
    // missing, instead of letting the first attempt fail.
    refreshers.borrow_mut().push(Box::new({
        let (state, model_row) = (state.clone(), model_row.clone());
        move || {
            let cfg = state.cfg.borrow();
            model_row.set_subtitle(if models::is_installed(cfg.engine, &cfg.model) {
                ""
            } else {
                "Not downloaded yet"
            });
        }
    }));
    group.add(&model_row);

    let labels: Vec<&str> = LANGUAGES.iter().map(|(_, label)| *label).collect();
    let selected = LANGUAGES.iter().position(|(code, _)| *code == cfg.language).unwrap_or(0);
    let language_row = combo("Language", &labels, selected as u32);
    language_row.set_subtitle("Pinning the language is the single biggest quality gain");
    language_row.connect_selected_notify({
        let state = state.clone();
        move |row| {
            let code = LANGUAGES[row.selected() as usize].0;
            state.update_cfg(|c| c.language = code.to_string());
        }
    });
    group.add(&language_row);

    let mic_row = adw::ActionRow::builder()
        .title("Microphone")
        .subtitle("System default input, chosen in Settings › Sound")
        .build();
    group.add(&mic_row);

    // GNOME owns the key combination, through the portal, so the trigger shown
    // here is the one the portal reported, not the one in the config file.
    let shortcut_row = adw::ActionRow::builder()
        .title("Dictation shortcut")
        .subtitle(shortcut_subtitle(&state.shortcut.borrow()))
        .build();
    shortcut_row.add_suffix(&button(
        "Change",
        Some("Opens Settings › Apps › Susurre › Global Shortcuts"),
        |button| {
            let window = button.root().and_downcast::<adw::PreferencesWindow>();
            open_app_settings(window.as_ref());
        },
    ));
    *state.shortcut_row.borrow_mut() = Some(shortcut_row.clone());
    group.add(&shortcut_row);

    // Left free rather than derived from the language: the application assumes
    // no language of its own.
    let prompt_row = adw::EntryRow::builder()
        .title("Decoder prompt")
        .text(&cfg.prompt)
        .show_apply_button(true)
        .build();
    prompt_row.connect_apply({
        let state = state.clone();
        move |row| {
            let prompt = row.text().to_string();
            state.update_cfg(|c| c.prompt = prompt);
        }
    });
    group.add(&prompt_row);

    group
}

/// Asks GNOME Settings to show this application's own page, which carries the
/// "Global Shortcuts" dialog.
///
/// Re-binding through the portal is not an option: GNOME keeps the binding per
/// application and answers `BindShortcuts` with the stored trigger without ever
/// showing a picker, so the button used to do nothing visible. A sandboxed
/// application's shortcuts are not listed in the Keyboard panel either; they
/// belong to the app's page under Applications.
fn open_app_settings(window: Option<&adw::PreferencesWindow>) {
    let app = format!("{APP_ID}.desktop").to_variant();
    let panel = ("applications", vec![app]).to_variant();
    let args = (
        "launch-panel",
        vec![panel],
        std::collections::HashMap::<String, glib::Variant>::new(),
    )
        .to_variant();

    let bus = match gtk::gio::bus_get_sync(gtk::gio::BusType::Session, gtk::gio::Cancellable::NONE)
    {
        Ok(bus) => bus,
        Err(e) => return log::error!("session bus: {e}"),
    };
    let window = window.cloned();
    bus.call(
        Some("org.gnome.Settings"),
        "/org/gnome/Settings",
        "org.freedesktop.Application",
        "ActivateAction",
        Some(&args),
        None,
        gtk::gio::DBusCallFlags::NONE,
        -1,
        gtk::gio::Cancellable::NONE,
        move |res| {
            if let Err(e) = res {
                log::error!("opening the keyboard panel: {e}");
                // Other desktops, or GNOME Settings not installed: the user can
                // still get there by hand.
                if let Some(window) = window {
                    window.add_toast(adw::Toast::new(
                        "Change it in Settings › Apps › Susurre › Global Shortcuts",
                    ));
                }
            }
        },
    );
}

/// Editable table of text replacements. Rows are never rebuilt while the user
/// types: every keystroke collects the whole table and writes it out, so
/// renaming a phrase cannot lose the row it belongs to.
fn replacements_group(state: &Rc<State>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Text replacement")
        .description(
            "Spoken phrase swapped for literal text anywhere in the transcript. \
             Use \\n for a line break.",
        )
        .build();

    let rows: Rows = Rc::new(RefCell::new(Vec::new()));
    let save: Rc<dyn Fn()> = Rc::new({
        let (state, rows) = (state.clone(), rows.clone());
        move || {
            let entries: BTreeMap<String, String> = rows
                .borrow()
                .iter()
                .map(|(_, phrase, value)| {
                    (phrase.text().trim().to_string(), text::unescape(&value.text()))
                })
                .filter(|(phrase, _)| !phrase.is_empty())
                .collect();
            state.update_cfg(|c| c.replacements = entries);
        }
    });

    for (phrase, value) in state.cfg.borrow().replacements.clone() {
        add_replacement_row(&group, &rows, &save, &phrase, &text::escape(&value));
    }

    let add_row = adw::ActionRow::new();
    add_row.add_prefix(&button("Add a replacement", None, {
        let (group, rows, save, add_row) =
            (group.clone(), rows.clone(), save.clone(), add_row.clone());
        move |_| {
            // A group only appends, so the button is taken out and put back to
            // keep it below the rows.
            group.remove(&add_row);
            add_replacement_row(&group, &rows, &save, "", "");
            group.add(&add_row);
        }
    }));
    group.add(&add_row);
    group
}

type Rows = Rc<RefCell<Vec<(adw::PreferencesRow, gtk::Entry, gtk::Entry)>>>;

fn add_replacement_row(
    group: &adw::PreferencesGroup,
    rows: &Rows,
    save: &Rc<dyn Fn()>,
    phrase: &str,
    value: &str,
) {
    let phrase = gtk::Entry::builder()
        .text(phrase)
        .placeholder_text("spoken phrase")
        .hexpand(true)
        .build();
    let target = gtk::Entry::builder()
        .text(value)
        .placeholder_text("replacement text")
        .hexpand(true)
        .build();
    let remove = icon_button("user-trash-symbolic", "Remove");

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&phrase);
    content.append(&gtk::Label::new(Some("\u{2192}")));
    content.append(&target);
    content.append(&remove);

    let row = adw::PreferencesRow::builder().activatable(false).build();
    row.set_child(Some(&content));
    group.add(&row);
    rows.borrow_mut().push((row.clone(), phrase.clone(), target.clone()));

    for entry in [&phrase, &target] {
        entry.connect_changed({
            let save = save.clone();
            move |_| save()
        });
    }

    remove.connect_clicked({
        let (group, rows, save, row) = (group.clone(), rows.clone(), save.clone(), row.clone());
        move |_| {
            rows.borrow_mut().retain(|(r, _, _)| r != &row);
            group.remove(&row);
            save();
        }
    });
}

fn models_group(
    state: &Rc<State>,
    window: &adw::PreferencesWindow,
    refreshers: &Refreshers,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Models")
        .description("Downloaded from Hugging Face, stored locally")
        .build();
    for info in models::CATALOG {
        group.add(&model_row(state, window, refreshers, info));
    }
    group
}

fn model_row(
    state: &Rc<State>,
    window: &adw::PreferencesWindow,
    refreshers: &Refreshers,
    info: &'static ModelInfo,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(info.label).build();
    let progress = gtk::ProgressBar::builder()
        .valign(gtk::Align::Center)
        .width_request(120)
        .visible(false)
        .build();
    let download = icon_button("folder-download-symbolic", "Download");
    let update = icon_button("view-refresh-symbolic", "Download again");
    let delete = icon_button("user-trash-symbolic", "Delete");
    delete.add_css_class("destructive-action");

    row.add_suffix(&progress);
    for b in [&download, &update, &delete] {
        row.add_suffix(b);
    }

    let buttons = [download.clone(), update.clone(), delete.clone()];
    refreshers.borrow_mut().push(Box::new({
        let (state, row, progress) = (state.clone(), row.clone(), progress.clone());
        move || {
            if progress.is_visible() {
                return;
            }
            let installed = models::is_installed(state.cfg.borrow().engine, info.id);
            download.set_visible(!installed);
            update.set_visible(installed);
            delete.set_visible(installed);
            row.set_subtitle(&if installed {
                format!("installed · about {} MB", info.size_mb)
            } else {
                format!("about {} MB", info.size_mb)
            });
        }
    }));

    for button in [&buttons[0], &buttons[1]] {
        button.connect_clicked({
            let (state, window) = (state.clone(), window.clone());
            let (refreshers, progress) = (refreshers.clone(), progress.clone());
            let buttons = buttons.clone();
            move |_| {
                for b in &buttons {
                    b.set_visible(false);
                }
                progress.set_fraction(0.0);
                progress.set_visible(true);
                fetch_model(&state, &window, &refreshers, &progress, info);
            }
        });
    }

    buttons[2].connect_clicked({
        let (state, window, refreshers) = (state.clone(), window.clone(), refreshers.clone());
        move |_| {
            if let Err(e) = models::delete(state.cfg.borrow().engine, info.id) {
                window.add_toast(adw::Toast::new(&format!("Failed: {e}")));
            }
            engine::unload();
            refresh_all(&refreshers);
        }
    });

    row
}

/// Downloads off the main thread, progress comes back through a channel.
fn fetch_model(
    state: &Rc<State>,
    window: &adw::PreferencesWindow,
    refreshers: &Refreshers,
    progress: &gtk::ProgressBar,
    info: &'static ModelInfo,
) {
    let engine = state.cfg.borrow().engine;
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        let report = {
            let tx = tx.clone();
            move |fraction: f64| {
                let _ = tx.try_send(Ok(fraction));
            }
        };
        let result = models::download(engine, info.id, &report);
        let _ = tx.send_blocking(result.map(|_| 1.0).map_err(|e| e.to_string()));
    });

    let (window, progress, refreshers) = (window.clone(), progress.clone(), refreshers.clone());
    glib::spawn_future_local(async move {
        let mut error = None;
        while let Ok(message) = rx.recv().await {
            match message {
                Ok(fraction) => progress.set_fraction(fraction),
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        progress.set_visible(false);
        if let Some(e) = error {
            window.add_toast(adw::Toast::new(&format!("Download failed: {e}")));
        }
        refresh_all(&refreshers);
    });
}

fn button<F>(label: &str, tooltip: Option<&str>, on_click: F) -> gtk::Button
where
    F: Fn(&gtk::Button) + 'static,
{
    let button = gtk::Button::builder()
        .label(label)
        .valign(gtk::Align::Center)
        .build();
    button.set_tooltip_text(tooltip);
    button.connect_clicked(on_click);
    button
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}
