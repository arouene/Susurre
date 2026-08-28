use crate::Cmd;
use anyhow::{anyhow, Result};
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut, Shortcut, ShortcutsChanged};
use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop};
use ashpd::desktop::{PersistMode, Session};
use ashpd::WindowIdentifier;
use futures_util::StreamExt;
use gtk::gdk::prelude::DisplayExtManual;
use gtk::glib::translate::FromGlib;

pub const SHORTCUT_ID: &str = "dictate";

/// Registers the shortcut with the portal and pushes key events into `tx`.
///
/// Binding only happens when nothing is registered yet. GNOME stores the
/// binding per application and, once it exists, answers `BindShortcuts` with
/// the stored trigger without ever showing a picker, so calling it again is not
/// a way to let the user change the key; the settings window points at GNOME's
/// own page under Settings > Apps instead.
pub async fn listen_shortcut(trigger: String, tx: async_channel::Sender<Cmd>) -> Result<()> {
    let proxy = GlobalShortcuts::new().await?;
    let session = proxy.create_session().await?;

    // One stream for every signal of the interface, not one per signal name.
    // Two separate streams have two separate queues, and select! picks between
    // them at random: a queued Activated could be delivered after the
    // Deactivated of the same press, starting a capture nobody asked for. The
    // next real press was then swallowed by the "already recording" guard and
    // the release looked lost. A single stream keeps the arrival order.
    let mut signals = proxy.receive_all_signals().await?;

    let existing = proxy.list_shortcuts(&session).await?.response()?;
    log::info!("shortcuts already registered: {:?}", existing.shortcuts());
    let bound = if existing.shortcuts().is_empty() {
        let shortcut = NewShortcut::new(SHORTCUT_ID, "Dictate (hold)")
            .preferred_trigger(Some(trigger.as_str()));
        proxy
            .bind_shortcuts(&session, &[shortcut], &WindowIdentifier::default())
            .await?
            .response()?
            .shortcuts()
            .to_vec()
    } else {
        existing.shortcuts().to_vec()
    };
    report(&tx, &bound, &trigger).await;

    while let Some(msg) = signals.next().await {
        let member = msg.header().member().map(|m| m.to_string());
        let cmd = match member.as_deref() {
            Some("Activated") => Cmd::KeyDown,
            Some("Deactivated") => Cmd::KeyUp,
            // The user can also rebind from GNOME Settings; the portal tells us.
            Some("ShortcutsChanged") => {
                if let Ok(c) = msg.body().deserialize::<ShortcutsChanged>() {
                    report(&tx, c.shortcuts(), &trigger).await;
                }
                continue;
            }
            other => {
                log::debug!("ignored GlobalShortcuts signal {other:?}");
                continue;
            }
        };
        log::debug!("{member:?} -> {cmd:?}");
        // A vanished receiver: the session is over.
        if tx.send(cmd).await.is_err() {
            break;
        }
    }
    // The session must outlive the listening loop.
    let _ = session.close().await;
    Ok(())
}

/// Tells the rest of the application which key the compositor actually bound.
/// It is not necessarily the requested one, since the user can change it in
/// GNOME Settings, and the settings window must show what is really in effect.
async fn report(tx: &async_channel::Sender<Cmd>, shortcuts: &[Shortcut], fallback: &str) {
    let bound = shortcuts.iter().find(|s| s.id() == SHORTCUT_ID);
    let description = match bound {
        Some(s) if !s.trigger_description().is_empty() => s.trigger_description().to_string(),
        // GNOME leaves the description empty in some versions; the requested
        // trigger is then the best guess we have.
        Some(_) => fallback.to_string(),
        None => return,
    };
    log::info!("shortcut bound to {description:?}");
    let _ = tx.send(Cmd::ShortcutBound(description)).await;
}

/// Keyboard input through the RemoteDesktop portal (the only supported route
/// under Wayland/GNOME).
pub struct Typist {
    proxy: RemoteDesktop<'static>,
    session: Session<'static, RemoteDesktop<'static>>,
    pub restore_token: Option<String>,
}



impl Typist {
    pub async fn new(restore_token: Option<String>) -> Result<Self> {
        let proxy = RemoteDesktop::new().await?;
        let session = proxy.create_session().await?;
        proxy
            .select_devices(
                &session,
                DeviceType::Keyboard.into(),
                restore_token.as_deref(),
                PersistMode::ExplicitlyRevoked,
            )
            .await?;
        let devices = proxy
            .start(&session, &WindowIdentifier::default())
            .await?
            .response()?;
        if !devices.devices().contains(DeviceType::Keyboard) {
            return Err(anyhow!("the portal did not grant keyboard access"));
        }
        let restore_token = devices.restore_token().map(str::to_string);
        Ok(Self { proxy, session, restore_token })
    }

    /// Types the transcript into the focused application.
    ///
    /// Fallback for a session without IBus. Keysym injection is not layout
    /// independent, whatever the portal documentation suggests: mutter
    /// resolves each keysym to a keycode *in the current keyboard group* and
    /// silently drops what it cannot find. Rather than let a dictation lose
    /// its accents without a word, the characters the layout cannot produce
    /// are named and the dictation fails loudly.
    pub async fn type_text(&self, text: &str) -> Result<()> {
        let missing: String = text.chars().filter(|c| !is_typable(*c)).collect();
        if !missing.is_empty() {
            return Err(anyhow!(
                "the keyboard layout cannot type {missing:?}; \
                 this needs the IBus engine, which did not start"
            ));
        }
        self.press_keys(text).await
    }

    async fn press_keys(&self, text: &str) -> Result<()> {
        for ch in text.chars() {
            self.tap(keysym(ch)).await?;
        }
        Ok(())
    }

    async fn tap(&self, keysym: i32) -> Result<()> {
        self.proxy
            .notify_keyboard_keysym(&self.session, keysym, KeyState::Pressed)
            .await?;
        self.proxy
            .notify_keyboard_keysym(&self.session, keysym, KeyState::Released)
            .await?;
        Ok(())
    }

}

/// Fallback when there is no display to ask: every Latin layout carries ASCII.
fn is_ascii_typable(ch: char) -> bool {
    ch.is_ascii_graphic() || matches!(ch, ' ' | '\n' | '\t')
}

/// Can the compositor type this character right now?
///
/// GDK holds the keymap the compositor is currently using, since Wayland hands
/// over a single one and GNOME swaps it when the input source changes. That
/// makes this an exact oracle for whether notify_keyboard_keysym will land or
/// be dropped.
fn is_typable(ch: char) -> bool {
    // Return and Tab are on every keyboard, but their code points are control
    // characters that map to no keysym at all.
    if matches!(ch, '\n' | '\t') {
        return true;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return is_ascii_typable(ch);
    };
    // SAFETY: any u32 is a valid keyval; gdk::Key is a plain newtype over it.
    let keyval = unsafe { gtk::gdk::Key::from_glib(gtk::gdk::unicode_to_keyval(ch as u32)) };
    display.map_keyval(keyval).is_some_and(|keys| !keys.is_empty())
}

/// Character to X11 keysym: latin-1 range straight through, everything else as
/// a Unicode keysym (0x01000000 + code point). Whether the compositor can act
/// on the result is a separate question, answered by is_typable.
fn keysym(ch: char) -> i32 {
    match ch {
        '\n' => 0xff0d,          // Return
        '\t' => 0xff09,          // Tab
        '\u{20}'..='\u{ff}' => ch as i32,
        _ => 0x0100_0000 + ch as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_ascii_typable, keysym};

    /// The bug that sent Susurre to IBus: mutter has no keycode for an accent
    /// in a plain `us` layout, so keysym injection silently drops it. Without
    /// a display to ask, this is the conservative guess.
    #[test]
    fn accents_are_not_assumed_typable() {
        assert!("problem integre".chars().all(is_ascii_typable));
        for word in ["problème", "intègre", "garçon", "€"] {
            assert!(!word.chars().all(is_ascii_typable), "{word}");
        }
    }

    #[test]
    fn whitespace_stays_typable() {
        assert!("a b\tc\nd.".chars().all(is_ascii_typable));
    }

    #[test]
    fn keysyms() {
        assert_eq!(keysym('a'), 0x61);
        assert_eq!(keysym('é'), 0xe9);
        assert_eq!(keysym('\n'), 0xff0d);
        assert_eq!(keysym('€'), 0x0100_20ac);
    }
}
