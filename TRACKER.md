# Porting tracker

Every feature of the Electron desktop app (Aoide 1.17.3), and where it stands
in the Rust app. Built from a read of the Electron code on 2026-10-24.

**Status:** ⬜ not started · 🟨 in progress · ✅ done · — not porting

**Round:** **1** core listening (the first usable release) · **2** next · **3** later

## Accounts and server

| Feature | Round | Status | Notes |
|---|---|---|---|
| Sign in to Jellyfin (address, username, password) | 1 | ✅ | Address rules from the iOS app: LAN → http, otherwise https, subpaths kept. Checks the server is Jellyfin before sending the password. Exercised in the app against a local stand-in server, not yet a real Jellyfin. |
| Find servers on the LAN (UDP 7359) | 3 | ⬜ | |
| Sign out (never blocks on the network) | 1 | ✅ | The screen leaves at once; the last report and `/Sessions/Logout` go afterwards inside a 5 s grace period. Exercised in the app against a local stand-in server, not yet a real Jellyfin. |
| Token stored in the system keyring | 1 | 🟨 | Behind a `CredentialStore` trait (keyring crate). Seen working with the macOS Keychain; Secret Service on Linux not yet run |
| One account per local store; another account erases it on sign-in | 1 | 🟨 | `adopt_account` runs before anything reads the store; a store whose owner can't be known asks Keep or Erase. The erase path is the store's own tests, not yet seen in the app |
| Rescan library (Jellyfin scheduled task) | 3 | ⬜ | |

## Screens

| Feature | Round | Status | Notes |
|---|---|---|---|
| Home: resume grid (last 6 contexts, "Pick up from {device}") | 2 | ⬜ | |
| Home: shelves (most played, recently added/played/released, random albums, genres) | 1 | ✅ | Recently added, recently played and most played (the last two as the albums of the tracks), recently released, random albums, and a random dozen genres as chips. A shelf with nothing on it is left out. Seen in the app against the stand-in server |
| Home: Discover tile (`/aoide/discover`) | 2 | ⬜ | New: the Electron app has no Discover |
| Albums list and album detail (with finish rate) | 1 | 🟨 | Grid loaded a page at a time, sortable (title, artist, recently added, release date); detail with cover, artist, year, Play, Shuffle, tracks in disc and track order with "Disc 1", "Disc 2" headings on a multi-disc album (seen against the stand-in server). Finish rate not yet |
| Album artists, artists, artist detail (discography, top songs, finish rate) | 1 | 🟨 | Album artists as a grid; an artist's albums. Top songs and finish rate not yet |
| Genres list and detail | 1 | ✅ | A genre's albums through `ListQuery::genre_ids` (Jellyfin `genreIds`). Seen in the app against the stand-in server |
| Songs list | 1 | ✅ | |
| Favourites | 2 | ⬜ | |
| Collections (saved filters) | 3 | ⬜ | |
| Search: plain Jellyfin search, songs and albums | 1 | ✅ | Songs, albums and artists as you type, 300 ms debounce |
| Search by description (OpenRouter → filters) | 3 | ⬜ | Model only translates to filters; Jellyfin selects |
| Command palette | 3 | ⬜ | |
| Now Playing column: artwork, controls, Queue/Lyrics tabs | 1 | ✅ | Cover, Queue and Lyrics tabs; the transport is the player bar's |
| Full-screen player | 2 | ⬜ | |
| Player bar: transport, seek, shuffle, repeat, volume, output device | 1 | 🟨 | Transport, seek, shuffle, repeat, volume, Crossfade and Infinity switches. Output device picker is Round 2. Shuffle and repeat buttons not yet clicked through in the app |
| Sleep timer (end of song, end of album, timed) | 2 | ⬜ | |
| Context menus on songs, albums, artists, playlists, queue rows | 1 | ✅ | An action strip under song rows and album/playlist headers: Play Next, Play Last, Add to Playlist, Not Interested, Don't Count Plays, Favourite, Go to Album/Artist. Album and artist tiles (their `…`, or a right click) and Aoide playlist rows open the same actions with Play and Shuffle over the window, fetching the songs first. Queue rows have their own buttons. Clicked through against the stand-in server (the right click not tried) |
| Get info | 3 | ⬜ | |
| Settings | 1 | ✅ | Exactly the Round 1 rows below, saved in the config file |

## Playback

| Feature | Round | Status | Notes |
|---|---|---|---|
| Streaming via `/Audio/{id}/universal`, token as `ApiKey` | 1 | ✅ | Played in the app from a local stand-in server |
| Native decoding (FLAC, ALAC, MP3, AAC, Vorbis, WAV, AIFF) | 1 | 🟨 | symphonia: no transcode needed for ALAC, unlike Electron. Each format measured on fixtures (`tests/formats.rs`). FLAC whose STREAMINFO minimum is the short last block (flacenc) plays, by a workaround for symphonia 0.6.1. Opus: no decoder; the stream URL never offers it, so Jellyfin transcodes it to ADTS AAC, and a local Opus file fails as "unsupported format: Opus" |
| Transcoding option (bitrate, format) | 2 | ⬜ | |
| Gapless joins, sample-exact | 1 | 🟨 | What Electron called "Exact joins (experimental)", here always on. Sample-exact for WAV, AIFF, FLAC, ALAC; MP3 (LAME tag) and Vorbis join with no gap. AAC does not: symphonia keeps the priming and padding, 64 ms between Apple-encoded M4A tracks, 21 ms between Jellyfin's AAC transcodes |
| Crossfade (per-pair plan, 8 s / 4 s by tempo, parity table) | 1 | 🟨 | Setting wired to the player; not yet heard across a real pair |
| Keep albums whole | 1 | 🟨 | Setting wired to the player; not yet heard across a real pair |
| Level volume: −18 LUFS, only turn down, peak ≤ 0 dBFS | 1 | 🟨 | From `/aoide/audio-analysis`, fetched (two-second wait at most, 200 ahead) before tracks are queued; the rest of the queue is measured afterwards in batches of 200 as it grows and applied to the queued tracks. Not yet run against a real sidecar |
| ReplayGain tags (album preferred) | 2 | ⬜ | |
| Skip silence (`/aoide/sound-bounds`, ignore < 300 ms) | 1 | 🟨 | From `/aoide/sound-bounds`, fetched with the loudness, late batches included; not yet run against a real sidecar |
| Output device picker | 2 | ⬜ | Following the default device is done: a failed stream reopens on the (new) default and plays on from the heard position, queue intact |
| Equalizer (parametric, presets, preamp) | 3 | ⬜ | |
| Compressor (threshold, ratio, attack, release, makeup, knee) | 3 | ⬜ | |
| Visualizer | 3 | ⬜ | Electron used audioMotion and Butterchurn; a native one is needed |
| Fades on pause and resume | 2 | ⬜ | |
| Auto DJ (beat-matched mixing) | — | — | Shelved on both apps (docs/arrangement.md in the iOS repo) |

## Queue

| Feature | Round | Status | Notes |
|---|---|---|---|
| Queue with Played / Now Playing / Next Up / Then headings | 1 | ✅ | |
| Manual lane: Play Next / Play Last ahead of the running album | 1 | 🟨 | Play Next and Play Last wired from the action strip; not yet clicked through in the app |
| Shuffle and repeat (all, one) | 1 | 🟨 | Wired to the player bar; not yet clicked through in the app |
| Reorder, remove, clear | 1 | ✅ | Up and down buttons within a section, remove, Clear (everything after the current track) |
| Queue saved and restored at launch | 1 | ✅ | Restored held at the saved track and position; follows its track if others left the server |

## Desktop integration

| Feature | Round | Status | Notes |
|---|---|---|---|
| MPRIS (bus name `aoide`): play, pause, next, previous, stop, volume, shuffle, loop, position, metadata | 1 | 🟨 | Wired (`integration.rs`): aoide-mpris on its own small tokio runtime, commands as a subscription, what plays published on every change with the cached cover as a `file://` URL (never the server's). The mapping is unit-tested and the Linux code cross-checked with clippy; not yet run on a Linux desktop. On macOS it reports itself unavailable and the app runs on |
| Global media hotkeys outside MPRIS | 3 | ⬜ | |
| Tray: play/pause, next, previous, stop, open, quit; minimise/exit to tray, start minimised | 2 | ⬜ | |
| Prevent sleep / suspend while playing | 2 | 🟨 | Came with MPRIS: aoide-mpris's inhibitor is set whenever playing starts or stops. Not yet run on a Linux desktop |
| Discord Rich Presence (Aoide's application id, options as in Electron) | 2 | ⬜ | |
| Single instance (second launch focuses the window) | 1 | 🟨 | Decided in `main.rs` before any window: a second launch calls Raise on the first (un-minimise and focus) and exits 0. Not yet run on a Linux desktop; macOS has no D-Bus, so every launch is first there |
| Window size and position remembered | 1 | ✅ | |
| Track info in the window title | 2 | ⬜ | |
| Accent colour and light/dark/system appearance | 2 | ⬜ | |
| Custom CSS | — | — | No CSS in a native app; theming is the accent colour |

## Reporting and history

| Feature | Round | Status | Notes |
|---|---|---|---|
| Report playback to Jellyfin (playing, progress, stopped) | 1 | ✅ | Started, progress every 10 s and on pause/resume, stopped. Exercised in the app against a local stand-in server, not yet a real Jellyfin. |
| Private mode (stop reporting) | 2 | ⬜ | |
| Local play history: events opened/closed, completed/skipped classification | 1 | ✅ | Opened at the heard start, closed with the furthest point reached, classified by the store; Don't Count Plays opens none |
| Activity tag on plays (Gaming, Focus, Chores, Commute) | 3 | ⬜ | |
| Finish rate on album and artist headers | 2 | ⬜ | |
| Replay (month, year, all time; top songs, artists, albums) | 2 | ⬜ | |

## Discovery

| Feature | Round | Status | Notes |
|---|---|---|---|
| Infinity: `/aoide/next`, local taste ranking as fallback | 1 | 🟨 | Fires once when fewer than 3 are left; factor summary on queue rows. The Instant Mix fallback (404) is seen working in the app; the sidecar path only against test doubles |
| Infinity albums mode | 3 | ⬜ | |
| Track, album and artist radio (Instant Mix) | 2 | ⬜ | |
| Taste flags: Not Interested, Don't Count Plays; "Hidden from mixes" list | 1 | ✅ | Set from the action strip; Hidden from mixes lists them in Settings with Clear |
| Mixes (describe a mood → smart rules via OpenRouter) | 3 | ⬜ | |
| Discover | 2 | ⬜ | |

## Playlists

| Feature | Round | Status | Notes |
|---|---|---|---|
| Jellyfin playlists: create, edit, delete, reorder, add/remove songs | 2 | ⬜ | |
| Jellyfin playlist cover upload | 3 | ⬜ | |
| Aoide playlists: create, rename, notes, delete, add, remove, move (fractional index) | 1 | ✅ | In the app: create, rename, delete, add songs; Edit shows the notes editor and each entry's Up, Down and Remove. A move is the store's `move_item`, one row written, measured past entries the server no longer has. Clicked through against the stand-in server; syncing the edits to a real sidecar not yet seen |
| Add to Aoide playlist (with search past 8, "New playlist…") | 1 | ✅ | |
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
| Triggers: launch, focus, at most once a minute; "Sync now" | 1 | 🟨 | Launch and window focus, at most once a minute; Sync Now in Settings starts one at once unless one is running, and Settings says the last outcome in words ("Synced: sent 34 changes."). Seen against the stand-in server's empty sync log; not yet run against a real sidecar |
| Sync status panel | 2 | ⬜ | |
| Handoff: publish this device's queue; pick up another's | 2 | ⬜ | Electron only published while one page was open; publish always |

## Lyrics

| Feature | Round | Status | Notes |
|---|---|---|---|
| Synced lyrics from Jellyfin, follow along, tap to seek | 1 | ✅ | |
| Lyric timing offset | 1 | ✅ | The Settings slider (±1.5 s); applied to line selection and to click-to-seek |
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
