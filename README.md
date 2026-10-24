# Aoide for Linux

A native Linux client for Jellyfin, written in Rust with [iced](https://iced.rs).
The sibling of the Aoide iPhone app, sharing its server-side companion, the
Aoide sidecar plugin, for sync, Infinity and Discover.

Written from scratch: no code from Feishin or the Electron-based Aoide desktop
app is copied here.

## Building

```
cargo run -p aoide-app
```

Linux needs ALSA and D-Bus development headers (`alsa-lib` and `dbus` on Arch).

## Layout

| crate | what |
|---|---|
| `aoide-jellyfin` | Jellyfin HTTP client |
| `aoide-sidecar` | Aoide sidecar client: sync, Infinity, Discover, measurements |
| `aoide-store` | local SQLite store: history, playlists, taste flags, op log |
| `aoide-audio` | decoding, output, gapless, crossfade, loudness |
| `aoide-mpris` | the Linux desktop: MPRIS media controls, single instance, sleep inhibit |
| `aoide-app` | the iced application |

What is ported from the Electron app, and what is still to come, is in
[TRACKER.md](TRACKER.md).

## Licence

GPL-3.0-or-later. The bundled Inter font (`crates/aoide-app/fonts`) is
under the SIL Open Font License 1.1; its licence is beside it.
