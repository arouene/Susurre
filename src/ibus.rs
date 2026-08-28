//! Text insertion through IBus.
//!
//! The portal route cannot type what the keyboard layout does not carry:
//! mutter resolves each keysym in the current group and drops the rest, so
//! dictating French on a `us` layout loses every accent. An input method does
//! not simulate keys at all, it *commits text*, which is why this path works
//! in any language and any application whatever the layout.
//!
//! Susurre registers its engine at runtime with `RegisterComponent` rather
//! than shipping the usual `share/ibus/component/*.xml`. That file would be
//! invisible from a Flatpak, whose export list carries no `share/ibus`, and
//! its `<exec>` would name a path inside the sandbox that the daemon cannot
//! run. Registering from the already running process sidesteps both, and the
//! registration disappears with the process.
//!
//! The engine is a strict passthrough: `ProcessKeyEvent` always returns false,
//! so the user keeps their own layout while it is selected. It is only made
//! the global engine for the moment of a commit, then the previous one is put
//! back, which also avoids needing to appear in Settings' input source list:
//! a runtime registration lands in `ListActiveEngines`, never in the static
//! registry that list is built from.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::time::Duration;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, StructureBuilder, Value};
use gtk::glib;
use zbus::{interface, Connection};

const ENGINE_NAME: &str = "susurre";
const COMPONENT_NAME: &str = "org.freedesktop.IBus.Susurre";
const FACTORY_PATH: &str = "/org/freedesktop/IBus/Factory";
const ENGINE_PATH: &str = "/org/freedesktop/IBus/Engine/Susurre";
const IBUS_NAME: &str = "org.freedesktop.IBus";
const IBUS_PATH: &str = "/org/freedesktop/IBus";

/// How long to wait for the daemon to build the engine and hand it the focus
/// after the engine is made global.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// The factory the daemon calls to obtain an engine. Ours is a single
/// long-lived object, so it always answers with the same path.
struct Factory;

#[interface(name = "org.freedesktop.IBus.Factory")]
impl Factory {
    fn create_engine(&self, _name: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        ObjectPath::try_from(ENGINE_PATH)
            .map(Into::into)
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
}

/// Passthrough engine. Every method the daemon may call is answered, because
/// an unimplemented one comes back as an error and the daemon gives up on the
/// engine; none of them does anything.
struct Engine {
    /// Signalled once the daemon has enabled the engine or given it the focus,
    /// which is when a commit can actually land somewhere.
    ready: async_channel::Sender<()>,
}

impl Engine {
    fn became_usable(&self) {
        let _ = self.ready.try_send(());
    }
}

#[interface(name = "org.freedesktop.IBus.Engine")]
impl Engine {
    /// False for every key: the application receives it untouched, so the
    /// user's own keyboard layout keeps working while Susurre is the engine.
    fn process_key_event(&self, _keyval: u32, _keycode: u32, _state: u32) -> bool {
        false
    }

    fn focus_in(&self) {
        self.became_usable();
    }

    fn focus_in_id(&self, _object_path: &str, _client: &str) {
        self.became_usable();
    }

    fn enable(&self) {
        self.became_usable();
    }

    fn focus_out(&self) {}
    fn focus_out_id(&self, _object_path: &str) {}
    fn disable(&self) {}
    fn reset(&self) {}
    fn page_up(&self) {}
    fn page_down(&self) {}
    fn cursor_up(&self) {}
    fn cursor_down(&self) {}
    fn set_capabilities(&self, _caps: u32) {}
    fn set_cursor_location(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    fn property_activate(&self, _name: &str, _state: u32) {}
    fn property_show(&self, _name: &str) {}
    fn property_hide(&self, _name: &str) {}
    fn candidate_clicked(&self, _index: u32, _button: u32, _state: u32) {}
    fn set_surrounding_text(&self, _text: Value<'_>, _cursor: u32, _anchor: u32) {}
    fn process_hand_writing_event(&self, _coordinates: Vec<f64>) {}
    fn cancel_hand_writing(&self, _strokes: u32) {}
    fn panel_extension_received(&self, _event: Value<'_>) {}
    fn panel_extension_register_keys(&self, _data: Value<'_>) {}

    // IBus declares this write-only; zbus 4 has no such thing, so a getter is
    // supplied and never consulted.
    #[zbus(property)]
    fn content_type(&self) -> (u32, u32) {
        (0, 0)
    }

    #[zbus(property)]
    fn set_content_type(&self, _value: (u32, u32)) {}

    #[zbus(property)]
    fn focus_id(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn active_surrounding_text(&self) -> bool {
        false
    }

    /// The whole point of this module.
    #[zbus(signal)]
    async fn commit_text(ctxt: &zbus::SignalContext<'_>, text: Value<'_>) -> zbus::Result<()>;
}

fn attachments() -> HashMap<String, OwnedValue> {
    HashMap::new()
}

fn ibus_text(text: &str) -> Value<'static> {
    let attributes = StructureBuilder::new()
        .add_field("IBusAttrList".to_string())
        .add_field(attachments())
        .add_field(Vec::<Value<'static>>::new())
        .build();
    StructureBuilder::new()
        .add_field("IBusText".to_string())
        .add_field(attachments())
        .add_field(text.to_string())
        .add_field(Value::from(attributes))
        .build()
        .into()
}

fn engine_description() -> Value<'static> {
    let mut builder = StructureBuilder::new()
        .add_field("IBusEngineDesc".to_string())
        .add_field(attachments())
        .add_field(ENGINE_NAME.to_string())
        .add_field("Susurre".to_string())
        .add_field("Local voice dictation".to_string())
        .add_field(String::new()) // language: none, the transcript decides
        .add_field("GPL-3.0-or-later".to_string())
        .add_field("Susurre".to_string())
        .add_field(crate::config::APP_ID.to_string()) // icon
        // "default" keeps whatever keyboard layout the user chose instead of
        // forcing one, which a passthrough engine has no business doing.
        .add_field("default".to_string())
        .add_field(0u32); // rank
    for _ in 0..8 {
        builder = builder.add_field(String::new());
    }
    builder.build().into()
}

fn component() -> Value<'static> {
    StructureBuilder::new()
        .add_field("IBusComponent".to_string())
        .add_field(attachments())
        .add_field(COMPONENT_NAME.to_string())
        .add_field("Susurre voice dictation".to_string())
        .add_field(env!("CARGO_PKG_VERSION").to_string())
        .add_field("GPL-3.0-or-later".to_string())
        .add_field("Susurre".to_string())
        .add_field(String::new()) // homepage
        // Empty on purpose: the process is already running and the daemon must
        // not try to spawn anything. This is what makes the Flatpak work.
        .add_field(String::new()) // command line
        .add_field("susurre".to_string()) // textdomain
        .add_field(Vec::<Value<'static>>::new()) // observed paths
        .add_field(vec![engine_description()])
        .build()
        .into()
}

/// Where the daemon's private bus is listening.
///
/// Not `glib::user_config_dir()`: inside the sandbox that points at the
/// application's own directory, whereas the address files are the host's,
/// bound in through `--filesystem=xdg-config/ibus:ro`. Several files pile up
/// across sessions, so the addresses are simply tried in turn.
fn addresses() -> Result<Vec<String>> {
    if let Ok(address) = std::env::var("IBUS_ADDRESS") {
        return Ok(vec![address]);
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    let dir = std::path::Path::new(&home).join(".config/ibus/bus");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {dir:?}"))? {
        let content = std::fs::read_to_string(entry?.path())?;
        if let Some(address) = content
            .lines()
            .find_map(|l| l.strip_prefix("IBUS_ADDRESS="))
        {
            found.push(address.to_string());
        }
    }
    if found.is_empty() {
        return Err(anyhow!("no IBus address under {dir:?}"));
    }
    Ok(found)
}

pub struct Ibus {
    connection: Connection,
    ready: async_channel::Receiver<()>,
}

impl Ibus {
    /// Connects to the daemon, publishes the engine and registers it.
    pub async fn connect() -> Result<Self> {
        let (tx, ready) = async_channel::bounded(1);
        let mut last = None;
        for address in addresses()? {
            match Self::open(&address, tx.clone()).await {
                Ok(connection) => {
                    log::info!("IBus engine registered");
                    return Ok(Self { connection, ready });
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("no reachable IBus daemon")))
    }

    async fn open(address: &str, ready: async_channel::Sender<()>) -> Result<Connection> {
        let connection = zbus::connection::Builder::address(address)?
            .serve_at(FACTORY_PATH, Factory)?
            .serve_at(ENGINE_PATH, Engine { ready })?
            .build()
            .await
            .with_context(|| format!("connecting to {address}"))?;
        connection
            .call_method(
                Some(IBUS_NAME),
                IBUS_PATH,
                Some(IBUS_NAME),
                "RegisterComponent",
                &(component(),),
            )
            .await
            .context("RegisterComponent")?;
        Ok(connection)
    }

    async fn global_engine(&self) -> Option<String> {
        let reply = self
            .connection
            .call_method(
                Some(IBUS_NAME),
                IBUS_PATH,
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(IBUS_NAME, "GlobalEngine"),
            )
            .await
            .ok()?;
        // The engine description is an IBus struct whose third field is its
        // name; the daemon answers with an error when nothing is active.
        let value: OwnedValue = reply.body().deserialize().ok()?;
        let structure = zbus::zvariant::Structure::try_from(value).ok()?;
        structure
            .fields()
            .get(2)
            .and_then(|f| String::try_from(f.try_clone().ok()?).ok())
    }

    async fn set_global_engine(&self, name: &str) -> Result<()> {
        self.connection
            .call_method(
                Some(IBUS_NAME),
                IBUS_PATH,
                Some(IBUS_NAME),
                "SetGlobalEngine",
                &(name,),
            )
            .await
            .with_context(|| format!("SetGlobalEngine({name})"))?;
        Ok(())
    }

    /// Inserts `text` into the focused application.
    pub async fn commit(&self, text: &str) -> Result<()> {
        let previous = self.global_engine().await;
        // Drain a stale readiness left by an earlier commit.
        while self.ready.try_recv().is_ok() {}

        let result = self.commit_once(text).await;

        if let Some(previous) = previous {
            if previous != ENGINE_NAME {
                if let Err(e) = self.set_global_engine(&previous).await {
                    log::warn!("could not restore the {previous} input method: {e}");
                }
            }
        }
        result
    }

    async fn commit_once(&self, text: &str) -> Result<()> {
        self.set_global_engine(ENGINE_NAME).await?;

        // The daemon builds the engine and hands it the focus asynchronously;
        // committing before that reaches nobody.
        let ready = self.ready.recv();
        let timeout = glib::timeout_future(READY_TIMEOUT);
        futures_util::pin_mut!(ready);
        futures_util::pin_mut!(timeout);
        if let futures_util::future::Either::Right(_) =
            futures_util::future::select(ready, timeout).await
        {
            return Err(anyhow!("the IBus engine never received the focus"));
        }

        let context = zbus::SignalContext::new(&self.connection, ENGINE_PATH)?;
        Engine::commit_text(&context, ibus_text(text))
            .await
            .context("CommitText")?;
        log::info!("committed {} characters through IBus", text.chars().count());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Type;

    /// The daemon rejects anything whose shape does not match its own
    /// serialisation, and the failure is a bare "Timeout was reached", so the
    /// signatures are pinned here.
    #[test]
    fn serialised_shapes_match_ibus() {
        assert_eq!(ibus_text("x").value_signature().to_string(), "(sa{sv}sv)");
        assert_eq!(
            engine_description().value_signature().to_string(),
            "(sa{sv}ssssssssussssssss)"
        );
        assert_eq!(
            component().value_signature().to_string(),
            "(sa{sv}ssssssssavav)"
        );
    }

    #[test]
    fn the_component_never_asks_the_daemon_to_spawn_anything() {
        let structure = zbus::zvariant::Structure::try_from(component()).unwrap();
        let command_line = structure.fields().get(8).unwrap();
        assert_eq!(String::try_from(command_line.try_clone().unwrap()).unwrap(), "");
        let _ = <(String, u32)>::signature();
    }
}
