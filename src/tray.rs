use crate::config::APP_ID;
use crate::Cmd;
use ksni::TrayMethods;

/// StatusNotifierItem icon. Under GNOME it only shows up with the
/// AppIndicator extension; without it the application stays reachable through
/// the "Background Applications" menu of the quick settings.
struct Tray {
    tx: async_channel::Sender<Cmd>,
}

impl Tray {
    fn send(&self, cmd: Cmd) {
        let _ = self.tx.try_send(cmd);
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        "Susurre".into()
    }

    fn icon_name(&self) -> String {
        APP_ID.into()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        vec![
            StandardItem {
                label: "Settings".into(),
                icon_name: "preferences-system-symbolic".into(),
                activate: Box::new(|t: &mut Tray| t.send(Cmd::OpenPreferences)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(|t: &mut Tray| t.send(Cmd::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// `disable_dbus_name`: the Flatpak sandbox cannot reserve the
/// org.kde.StatusNotifierItem-PID-N bus name.
pub fn spawn(tx: async_channel::Sender<Cmd>) {
    gtk::glib::spawn_future_local(async move {
        let tray = Tray { tx };
        if let Err(e) = tray.disable_dbus_name(true).spawn().await {
            log::warn!("no notification area available: {e}");
        }
    });
}
