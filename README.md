# Susurre

**Local dictation for GNOME. Hold a key, speak, and the text is typed into
whatever application you are using.**

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-GTK4%20%2F%20libadwaita-orange.svg)](https://gtk-rs.org)
[![Flatpak](https://img.shields.io/badge/Flatpak-GNOME%2049-4a86cf.svg)](build-aux/fr.arouene.Susurre.yaml)

Susurre runs Whisper on your own machine. No account, no API key, and no audio
leaves the computer. The only network traffic is downloading the speech model
you pick, once.

It is not a note taking app. It types into the field you already have focused,
whether that is an editor, a browser form, a chat window, or a terminal.

```
hold shortcut -> microphone -> voice activity -> Whisper -> your commands -> keystrokes
                 (PipeWire)      detection       (local)    (replacements)    (portal)
```

## Features

- **Two engines.** whisper.cpp for a single self contained binary, or
  faster-whisper / CTranslate2 when you want its Silero voice activity
  detection and beam search.
- **Model manager.** Download, update, and delete models from the settings
  window, with a progress bar. Models come from Hugging Face and live under
  `~/.var/app/fr.arouene.Susurre/data/susurre/models`.
- **Language agnostic.** Nothing in the application assumes a language. Pin one
  in the settings, or leave automatic detection on.
- **Text replacement.** Define your own substitutions, such as speaking "new
  line" to insert a line break. Matching ignores case and accents, so the
  phrase you type in the settings does not have to match the engine's spelling.
  The table is empty by default.
- **Any language, any layout.** Text is committed through an IBus engine
  rather than typed key by key, so accents and non Latin scripts land whatever
  keyboard layout is active.
- **Decoder prompt.** A free text hint that steers punctuation, casing, and
  vocabulary, useful for proper nouns and domain jargon.
- **Hallucination guards.** Silence is trimmed before decoding, quiet
  microphones are normalised, and the decoder runs deterministically so a noisy
  clip does not produce invented sentences.
- **GPU offload.** whisper.cpp runs on the GPU through Vulkan, enabled in the
  Flatpak build. `large-v3-turbo` decodes roughly ten times faster than on CPU,
  on any driver the runtime carries: radv, intel, nouveau.
- **Wayland native.** Global shortcut and keystroke injection both go through
  XDG desktop portals. No X11, no accessibility hacks, no root helper.
- **Background service.** Stays out of the way, registers for autostart, and
  exposes a StatusNotifierItem tray icon.

## Requirements

- Linux distribution with Flatpak (Tested on Fedora Silverblue 43)
- GNOME 49 or newer, on Wayland
- `xdg-desktop-portal-gnome`, which provides the GlobalShortcuts and
  RemoteDesktop portals
- Roughly 500 MB of disk for the default model, more for the larger ones

## Install

```sh
git clone https://github.com/arouene/susurre
cd susurre
./build-flatpak.sh
```

The script pulls `org.flatpak.Builder` and the manifest dependencies if they
are missing, then installs the application for the current user. It always uses
the `org.flatpak.Builder` Flatpak rather than a `flatpak-builder` binary found
on the machine, so the build behaves the same everywhere. Add
`--bundle` to also produce a distributable `susurre.flatpak`, or `--clean` to
rebuild from an empty cache.

```sh
flatpak run fr.arouene.Susurre
```

## First run

1. The settings window opens. The application keeps running in the background
   after you close it.
2. Under **Models**, download one. `small` is a reasonable starting point,
   `large-v3-turbo` is the best quality per second of CPU time.
3. GNOME asks you to pick the key combination the first time Susurre
   registers it. To change it later, use **Change** next to the shortcut, which
   opens Settings > Apps > Susurre > Global Shortcuts.
4. Hold the shortcut, speak, release. The first dictation asks for permission
   to inject keystrokes. Grant it once, and the restore token is saved.

To reopen the settings later: the **Susurre** icon in the app grid, or
`flatpak run fr.arouene.Susurre`, or Quick Settings > **Background
Apps**.

## Configuration

Everything in the settings window is written immediately to
`~/.var/app/fr.arouene.Susurre/config/susurre/config.toml`. The file is
re-read before every dictation, so hand edits take effect without a restart.

| Setting | Meaning |
| --- | --- |
| `engine` | `whisper-cpp` or `faster-whisper` |
| `model` | Model id, for example `small` or `large-v3-turbo` |
| `language` | ISO code such as `fr`, or `auto` |
| `shortcut` | Preferred trigger, for example `CTRL+ALT+d`. Only a request: GNOME owns the final binding, and the settings window shows what it actually bound |
| `prompt` | Decoder hint, in your own language |
| `replacements` | Table of spoken expression to replacement text |

### Punctuation and text replacement

Whisper punctuates and capitalises on its own. Do not speak "comma" or
"period", it writes the words. For insertions it will not produce, such as a
line break, define replacements yourself:

```toml
prompt = "Dictation with punctuation, capitals and accents."

[replacements]
"new line" = "\n"
"new paragraph" = "\n\n"
```

Matching ignores case and diacritics, requires whole words, and absorbs the
punctuation the engine appends to the expression. `"Hello. New line. How are
you?"` becomes `Hello.\nHow are you?`, and a phrase typed `a la ligne` still
matches a spoken `À la ligne`.

## Engines and models

| Model | Size on disk | Notes |
| --- | --- | --- |
| `tiny` | 75 MB | Fast, low accuracy |
| `base` | 142 MB | Fast, still weak on non English |
| `small` | 466 MB | Good default |
| `medium` | 1.5 GB | Noticeably better, several seconds per dictation without a GPU |
| `large-v3` | 3.1 GB | Best accuracy, slow without a GPU |
| `large-v3-turbo` | 1.6 GB | Close to `large-v3`, much faster |

Automatic language detection works from a few seconds of audio and is not very
reliable at that length. Pinning the language gives better results.

**whisper.cpp** is linked directly into the binary through `whisper-rs`. It has
no external process and loads a single `ggml-*.bin` file. `whisper-rs` 0.14
does not expose whisper.cpp's VAD, so Susurre applies its own energy based
trimming before decoding.

The Flatpak builds it with the `vulkan` feature, so the weights are offloaded to
the GPU. The runtime carries `libvulkan` and the mesa ICDs, the SDK carries the
headers and `glslc`; nothing has to be installed on the host beyond a working
mesa. Measured on a Quadro T2000, decoding an 11 s clip: `small` 7.2 s on CPU
against 1.6 s on Vulkan, `large-v3-turbo` 47.9 s against 4.9 s. The first launch
after an update spends about 77 s compiling shaders, then caches them.

**faster-whisper** runs as a short lived Python process against a CTranslate2
model directory. It brings Silero VAD, beam search, and `int8` quantised
inference.

## How it works

| Concern | Mechanism |
| --- | --- |
| Global shortcut | `org.freedesktop.portal.GlobalShortcuts`, push to talk on Activated and Deactivated |
| Inserting the result | An IBus engine registered at runtime, which commits text rather than simulating keys. Falls back to RemoteDesktop keysyms when IBus is absent |
| Autostart and background | `org.freedesktop.portal.Background` |
| Audio capture | `cpal`, downmixed to mono and resampled to 16 kHz |
| Tray icon | StatusNotifierItem through `ksni` |

Text is inserted through **IBus**, not through simulated keystrokes. An input
method commits text directly to the focused application, so dictation works in
any language and any keyboard layout.

Susurre registers its engine at runtime with `RegisterComponent` instead of
shipping `share/ibus/component/*.xml`. A Flatpak cannot export that directory,
and the `<exec>` such a file carries would name a path inside the sandbox that
the daemon cannot run; registering from the already running process sidesteps
both, and the registration disappears with the process. The engine is a strict
passthrough, `ProcessKeyEvent` always returns false, and it is only made the
global engine for the duration of a commit before the previous one is restored,
so your own input method and keyboard layout are left alone.

The RemoteDesktop portal remains as a fallback for a session without IBus. It
is not layout independent, whatever the portal documentation suggests: mutter
resolves each keysym to a keycode *in the current keyboard group* and silently
drops what it cannot find, logging `No keycode found for keyval e8 in current
group`. Susurre asks the keymap first, through `gdk_display_map_keyval`, and
refuses the dictation naming the offending characters rather than losing them
in silence.

### Source layout

| File | Responsibility |
| --- | --- |
| `src/main.rs` | Application lifecycle, command loop, dictation pipeline |
| `src/ibus.rs` | IBus engine: registration, passthrough, text commit |
| `src/portal.rs` | GlobalShortcuts listener and RemoteDesktop fallback typist |
| `src/audio.rs` | Capture, downmix, resample, VAD, gain normalisation |
| `src/engine/` | `Transcriber` trait, whisper.cpp and faster-whisper backends |
| `src/models.rs` | Catalogue, download, update, delete |
| `src/text.rs` | Text replacements |
| `src/ui.rs` | libadwaita settings window |
| `src/tray.rs` | StatusNotifierItem |
| `python/susurre-ct2.py` | faster-whisper helper process |

## Development

With `gtk4-devel` and `libadwaita-devel` installed, cargo works directly:

```sh
cargo build
cargo test
```

On Silverblue and other "atomics" system, those headers are not on the host, so
`dev.sh` runs the same cargo command inside the GNOME SDK:

```sh
./dev.sh build
./dev.sh test
./dev.sh clippy --all-targets
./dev.sh run
```

The SDK is needed once:

```sh
flatpak install flathub org.gnome.Sdk//49 org.gnome.Platform//49 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08 \
    org.freedesktop.Sdk.Extension.llvm20//25.08
```

Run with `RUST_LOG=susurre=debug` to trace portal signals, recording length,
and the transcribed text.

## Troubleshooting

**No tray icon.** GNOME 49 has no built in tray. Install the AppIndicator
extension, or reach the application through Quick Settings > Background Apps.

**The shortcut is not in Settings > Keyboard.** A sandboxed application's
global shortcuts are not listed there. They live in Settings > Apps > Susurre >
Global Shortcuts, which the **Change** button opens directly.

**The dictation does not stop when I release the shortcut.** Release the
letter before the modifier. mutter matches the release event against the whole
accelerator, so letting go of `Super` before `d` leaves the accelerator
unmatched and no `Deactivated` signal is ever sent. The capture then runs to its
120 s guard. Pressing and releasing the shortcut again stops it. This is
compositor side and cannot be worked around through the portal; the only
alternatives are reading `/dev/input` directly, which needs the `input` group
and a permission Flathub will not grant, or dropping push to talk for a toggle.

**GNOME asks for the key combination again.** Only on the first registration
for a given application id. Afterwards `BindShortcuts` returns the stored
trigger silently, which is also why that call cannot be used to re-open the
picker.

**Accents are dropped, and the log names them.** IBus did not start, so the
portal fallback took over, and it can only type what the keyboard layout
carries. Look for `IBus engine registered` at startup; `no IBus engine (...)`
says why it is missing.

**Nothing is typed.** Check the logs. A missing model reports
`model not downloaded` and raises a desktop notification. If the RemoteDesktop
permission was denied, delete `remote_desktop_token` from the config file and
try again.

**`Build directory ... not initialized, use flatpak build-init`.** A native
`flatpak-builder` was used instead of `org.flatpak.Builder`, and its
rofiles-fuse mount did not take. The build script no longer allows this. If you
call flatpak-builder by hand, use `flatpak run org.flatpak.Builder` instead.

**`Failed to check out cache` during the build.** The flatpak-builder cache is
corrupt, usually after an interrupted build or a project directory copied from
another machine. `--force-clean` does not touch that cache, so purge it:

```sh
./build-flatpak.sh --clean
```

The script also retries once on its own after purging.

**Susurre records from the wrong microphone.** It always uses the system default
input. Pick the input in Settings > Sound and PipeWire routes it.

## Flatpak permissions

Susurre asks for as little as it can. Everything that touches the desktop goes
through XDG portals, which are mediated by the user and need no manifest entry.

| Permission | Why |
| --- | --- |
| `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc` | Draw the settings window |
| `--device=dri` | GPU for the Vulkan backend |
| `--socket=pulseaudio` | Capture the microphone, through pipewire-pulse |
| `--share=network` | Download models from Hugging Face. Nothing else leaves the machine |
| `--talk-name=org.kde.StatusNotifierWatcher` | Publish the tray icon, when an AppIndicator extension provides the watcher |
| `--talk-name=org.gnome.Settings` | Let the **Change** button open Settings > Apps > Susurre |
| `--filesystem=xdg-cache/ibus` | The IBus private bus socket, to register the input engine |
| `--filesystem=xdg-config/ibus:ro` | The file naming that socket's address |

`flatpak info --show-permissions fr.arouene.Susurre` prints what is actually
installed, and Settings > Apps > Susurre lets you revoke any of it.

## Known limitations

- The Flatpak build downloads crates and Python wheels at build time. Flathub
  submission requires vendored sources through `flatpak-cargo-generator` and
  `flatpak-pip-generator`.
- No streaming transcription. Audio is decoded after you release the key.
- GPU offload covers whisper.cpp only, through Vulkan. faster-whisper runs on
  CPU, since CTranslate2 offers CUDA and nothing else.
- Releasing the modifier before the key loses the shortcut release. See
  Troubleshooting.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
