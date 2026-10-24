# Porting tracker

Every feature of the Electron desktop app (Aoide 1.17.3), and where it stands
in the Rust app. Built from a read of the Electron code on 2026-10-24.

**Status:** ⬜ not started · 🟨 in progress · ✅ done · — not porting

**Round:** **1** core listening (the first usable release) · **2** next · **3** later

## Accounts and server

| Feature | Round | Status | Notes |
|---|---|---|---|
| Sign in to Jellyfin (address, username, password) | 1 | 🟨 | Address rules from the iOS app: LAN → http, otherwise https, subpaths kept |
| Find servers on the LAN (UDP 7359) | 3 | ⬜ | |
| Sign out (never blocks on the network) | 1 | ⬜ | |
| Token stored in the system keyring | 1 | ⬜ | Secret Service; not in a plain file |
| One account per local store; another account erases it on sign-in | 1 | 🟨 | |
| Rescan library (Jellyfin scheduled task) | 3 | ⬜ | |

## Screens

| Feature | Round | Status | Notes |
|---|---|---|---|
| Home: resume grid (last 6 contexts, "Pick up from {device}") | 2 | ⬜ | |
| Home: shelves (most played, recently added/played/released, random albums, genres) | 1 | ⬜ | |
| Home: Discover tile (`/aoide/discover`) | 2 | ⬜ | New: the Electron app has no Discover |
| Albums list and album detail (with finish rate) | 1 | ⬜ | |
| Album artists, artists, artist detail (discography, top songs, finish rate) | 1 | ⬜ | |
| Genres list and detail | 1 | ⬜ | |
| Songs list | 1 | ⬜ | |
| Favourites | 2 | ⬜ | |
| Collections (saved filters) | 3 | ⬜ | |
| Search: plain Jellyfin search, songs and albums | 1 | ⬜ | |
| Search by description (OpenRouter → filters) | 3 | ⬜ | Model only translates to filters; Jellyfin selects |
| Command palette | 3 | ⬜ | |
| Now Playing column: artwork, controls, Queue/Lyrics tabs | 1 | ⬜ | |
| Full-screen player | 2 | ⬜ | |
| Player bar: transport, seek, shuffle, repeat, volume, output device | 1 | ⬜ | |
| Sleep timer (end of song, end of album, timed) | 2 | ⬜ | |
| Context menus on songs, albums, artists, playlists, queue rows | 1 | ⬜ | Play now/next/last, add to playlist, taste flags, favourite, go to |
| Get info | 3 | ⬜ | |
| Settings | 1 | ⬜ | Only the rows below |

## Playback

| Feature | Round | Status | Notes |
|---|---|---|---|
| Streaming via `/Audio/{id}/universal`, token as `ApiKey` | 1 | 🟨 | |
| Native decoding (FLAC, ALAC, MP3, AAC, Opus, Vorbis, WAV) | 1 | 🟨 | symphonia: no transcode needed for ALAC, unlike Electron |
| Transcoding option (bitrate, format) | 2 | ⬜ | |
| Gapless joins, sample-exact | 1 | 🟨 | What Electron called "Exact joins (experimental)", here always on |
| Crossfade (per-pair plan, 8 s / 4 s by tempo, parity table) | 1 | 🟨 | |
| Keep albums whole | 1 | 🟨 | |
| Level volume: −18 LUFS, only turn down, peak ≤ 0 dBFS | 1 | 🟨 | From `/aoide/audio-analysis`, 50 ahead |
| ReplayGain tags (album preferred) | 2 | ⬜ | |
| Skip silence (`/aoide/sound-bounds`, ignore < 300 ms) | 1 | 🟨 | |
| Output device picker | 2 | ⬜ | |
| Equalizer (parametric, presets, preamp) | 3 | ⬜ | |
| Compressor (threshold, ratio, attack, release, makeup, knee) | 3 | ⬜ | |
| Visualizer | 3 | ⬜ | Electron used audioMotion and Butterchurn; a native one is needed |
| Fades on pause and resume | 2 | ⬜ | |
| Auto DJ (beat-matched mixing) | — | — | Shelved on both apps (docs/arrangement.md in the iOS repo) |

## Queue

| Feature | Round | Status | Notes |
|---|---|---|---|
| Queue with Played / Now Playing / Next Up / Then headings | 1 | 🟨 | |
| Manual lane: Play Next / Play Last ahead of the running album | 1 | 🟨 | Ported from the iOS PlayQueue |
| Shuffle and repeat (all, one) | 1 | 🟨 | |
| Reorder, remove, clear | 1 | ⬜ | |
| Queue saved and restored at launch | 1 | ⬜ | |

## Desktop integration

| Feature | Round | Status | Notes |
|---|---|---|---|
| MPRIS (bus name `aoide`): play, pause, next, previous, stop, volume, shuffle, loop, position, metadata | 1 | ⬜ | Media keys come through MPRIS on Linux |
| Global media hotkeys outside MPRIS | 3 | ⬜ | |
| Tray: play/pause, next, previous, stop, open, quit; minimise/exit to tray, start minimised | 2 | ⬜ | |
| Prevent sleep / suspend while playing | 2 | ⬜ | |
| Discord Rich Presence (Aoide's application id, options as in Electron) | 2 | ⬜ | |
| Single instance (second launch focuses the window) | 1 | ⬜ | |
| Window size and position remembered | 1 | ⬜ | |
| Track info in the window title | 2 | ⬜ | |
| Accent colour and light/dark/system appearance | 2 | ⬜ | |
| Custom CSS | — | — | No CSS in a native app; theming is the accent colour |

## Reporting and history

| Feature | Round | Status | Notes |
|---|---|---|---|
| Report playback to Jellyfin (playing, progress, stopped) | 1 | 🟨 | |
| Private mode (stop reporting) | 2 | ⬜ | |
| Local play history: events opened/closed, completed/skipped classification | 1 | 🟨 | Same thresholds as the phone |
| Activity tag on plays (Gaming, Focus, Chores, Commute) | 3 | ⬜ | |
| Finish rate on album and artist headers | 2 | ⬜ | |
| Replay (month, year, all time; top songs, artists, albums) | 2 | ⬜ | |

## Discovery

| Feature | Round | Status | Notes |
|---|---|---|---|
| Infinity: `/aoide/next`, local taste ranking as fallback | 1 | 🟨 | Songs mode. Show the factors on queue rows (Electron keeps them but never shows them) |
| Infinity albums mode | 3 | ⬜ | |
| Track, album and artist radio (Instant Mix) | 2 | ⬜ | |
| Taste flags: Not Interested, Don't Count Plays; "Hidden from mixes" list | 1 | 🟨 | |
| Mixes (describe a mood → smart rules via OpenRouter) | 3 | ⬜ | |
| Discover | 2 | ⬜ | |

## Playlists

| Feature | Round | Status | Notes |
|---|---|---|---|
| Jellyfin playlists: create, edit, delete, reorder, add/remove songs | 2 | ⬜ | |
| Jellyfin playlist cover upload | 3 | ⬜ | |
| Aoide playlists: create, rename, notes, delete, add, remove, move (fractional index) | 1 | 🟨 | |
| Add to Aoide playlist (with search past 8, "New playlist…") | 1 | ⬜ | |
| Built-in Rediscover Mix and Heavy Rotation | 2 | 🟨 | |
| Smart playlists **evaluated** on open | 2 | 🟨 | The Electron app syncs the rules but does not evaluate them on the detail screen; do it right here |
| Covers: image hash → sidecar → artwork item → first track | 2 | ⬜ | |
| Repair covers | 3 | ⬜ | |
| Playlist import (Spotify link, CSV, Exportify; `/aoide/match`) | 3 | ⬜ | |
| Folders screen | 3 | ⬜ | Folders sync but the Electron app never had a screen |
| Sharing a playlist with another user | 3 | ⬜ | Client methods exist in Electron but no UI |

## Sync with the sidecar

| Feature | Round | Status | Notes |
|---|---|---|---|
| Push / pull, cursors, acceptedEntities gating | 1 | 🟨 | |
| Per-field merge for playlists, folders, track flags; row merge elsewhere | 1 | 🟨 | |
| Quarantine of unreadable or refused ops | 1 | 🟨 | |
| Covers uploaded before the op naming them | 2 | 🟨 | |
| Share-revoked edits held back, shares re-read every 5 min | 3 | ⬜ | |
| Triggers: launch, focus, at most once a minute; "Sync now" | 1 | ⬜ | |
| Sync status panel | 2 | ⬜ | |
| Handoff: publish this device's queue; pick up another's | 2 | ⬜ | Electron only published while one page was open; publish always |

## Lyrics

| Feature | Round | Status | Notes |
|---|---|---|---|
| Synced lyrics from Jellyfin, follow along, tap to seek | 1 | 🟨 | |
| Lyric timing offset | 1 | ⬜ | |
| Internet lyrics (LRCLIB, NetEase, Genius, SimpMusic), on by default, can be turned off | 2 | ⬜ | |
| Lyric translation | 3 | ⬜ | The iOS app does it on-device; needs a local option on Linux |
| Furigana and romaji | 3 | ⬜ | |

## Settings shown in round 1

Crossfade, Keep albums whole, Infinity on/off, Skip silence, Level volume,
Lyric offset, Hidden from mixes, account (server, user, Sign out). Everything
else in the Electron settings arrives with its feature above.

## Distribution

| Feature | Round | Status | Notes |
|---|---|---|---|
| pacman repository from GitHub releases (`[aoide]`) | 1 | ⬜ | Replaces the Electron package under the same app id `io.github.Virel0.Aoide` when ready |
| Flatpak | 2 | ⬜ | |
| AppImage / deb | 3 | ⬜ | |
| In-app "update available" notice | 3 | ⬜ | pacman and Flatpak update the app themselves |
| Settings import/export, log level, clear cache | 3 | ⬜ | |

## Not porting

MPV and jukebox; internet radio; folder browser; tag editor; remote-control
server; share links; ratings; Navidrome and Subsonic; external links and
Last.fm; lyric provider picker; hotkey rebinding; theme gallery, fonts, zoom,
backgrounds; the 32 bundled translations (English only, French later on both
apps); playback speed and pitch; Feishin's fixed crossfade; analytics; Windows
and macOS builds.
