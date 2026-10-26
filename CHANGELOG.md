# Changelog

Notable changes to Aoide for Linux. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). [TRACKER.md](TRACKER.md) has the
state of every feature in more detail.

## [Unreleased]

### Changed
- The AppImage and deb workflow can be run by hand for a given tag.
- README, security policy, contributing guide, issue and pull request
  templates.

## [0.1.0] - 2026-10-25

The first release, for testing beside the Electron app: browsing, gapless
playback, the queue, sync with the Aoide sidecar, Infinity and Discover, synced
lyrics with translation by your own LibreTranslate server, the equaliser, media
keys and the tray.

### Added

**Accounts and server**
- Sign in to Jellyfin, with the iPhone app's address rules (home addresses over
  HTTP, the rest over HTTPS), and a search for servers on the local network.
- The token kept in the system keyring: the Secret Service through oo7, or the
  Secret portal inside a Flatpak.
- Sign-out that never waits for the network; one account per local store.
- Rescan library, for a server's administrators.

**Library**
- Home with Jump back in, Pick up from another device, the Discover tile and
  shelves; albums, artists, songs, genres, favourites and collections.
- Search as you type, search by description through OpenRouter, and a command
  palette on Ctrl+K.
- Get info on any song.

**Playback**
- Native decoding with symphonia (FLAC, ALAC, MP3, AAC, Vorbis, WAV, AIFF) and
  sample-exact gapless joins for WAV, AIFF, FLAC and ALAC.
- Crossfade, Keep albums whole, Level volume (sidecar measurements, then
  ReplayGain tags), Skip silence and fades on pause and resume.
- Equaliser with 14 presets and your own, compressor, output device picker,
  transcoding option, sleep timer, private listening, full-screen player and
  visualiser.
- The queue with Played, Now playing, Next up and Then, saved and restored at
  launch.

**Lyrics**
- Synced lyrics from Jellyfin with click to seek and a timing offset.
- LRCLIB when the server has none, cached for 30 days.
- Translation by your own LibreTranslate server.
- Romaji and furigana for Japanese lyrics.

**Playlists, history and discovery**
- Aoide playlists with folders, notes, covers and sharing; Jellyfin playlists.
- Smart playlists evaluated on open; Rediscover Mix and Heavy Rotation.
- Playlist import from a Spotify link or a CSV file.
- Local play history, Replay, finish rates, taste flags and activity tags.
- Infinity (songs or whole albums), Discover, radio and mixes.

**Sync**
- Push and pull with the Aoide sidecar, per-field merge, quarantine of refused
  changes, a sync status panel, and handoff of the queue between devices.

**Desktop**
- MPRIS, tray, single instance and sleep inhibit; Discord Rich Presence;
  appearance and accent colour; the song in the window title; an update notice;
  settings export and import, log level and cache clearing.

**Distribution**
- A pacman repository, a Flatpak bundle, an AppImage and a deb, built by GitHub
  Actions and attached to the release with SHA-256 checksums.

### Security
- The token, the LibreTranslate key and the lyrics follow redirects only within
  their server's own scheme, host and port.
- Answers from Jellyfin, the sidecar, artwork and LibreTranslate are capped in
  size, and a stream's temporary file has a ceiling.
- The Discord socket must belong to the user running the app.

### Known limits
- Not yet run on a Linux desktop: MPRIS, the tray, single instance and the
  keyring outside CI; the Flatpak has been built but not yet installed.
- Releases are not signed yet; see [docs/releases.md](docs/releases.md).
- iced 0.14 gives buttons and tiles no keyboard focus and exposes no
  screen-reader tree. Ctrl+K reaches every screen.
- English only.

[Unreleased]: https://github.com/Virel0/Aoide-Linux/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Virel0/Aoide-Linux/releases/tag/v0.1.0
