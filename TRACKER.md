# Porting tracker

Every feature of the Electron desktop app (Aoide 1.17.3), and where it stands
in the Rust app. Built from a read of the Electron code on 2026-10-24.

**Status:** ⬜ not started · 🟨 in progress · ✅ done · — not porting

**Round:** **1** core listening (the first usable release) · **2** next · **3** later

## Accounts and server

| Feature | Round | Status | Notes |
|---|---|---|---|
| Sign in to Jellyfin (address, username, password) | 1 | ✅ | Address rules from the iOS app: LAN → http, otherwise https, subpaths kept. Checks the server is Jellyfin before sending the password. Exercised in the app against a local stand-in server, not yet a real Jellyfin. |
| Find servers on the LAN (UDP 7359) | 3 | 🟨 | library done; UI pending. `discover_servers` in aoide-jellyfin: the one probe, answers gathered for a short wait; tested against a local UDP responder |
| Sign out (never blocks on the network) | 1 | ✅ | The screen leaves at once; the last report and `/Sessions/Logout` go afterwards inside a 5 s grace period. Exercised in the app against a local stand-in server, not yet a real Jellyfin. |
| Token stored in the system keyring | 1 | 🟨 | Behind a `CredentialStore` trait (keyring crate). Seen working with the macOS Keychain; Secret Service on Linux not yet run |
| One account per local store; another account erases it on sign-in | 1 | 🟨 | `adopt_account` runs before anything reads the store; a store whose owner can't be known asks Keep or Erase. The erase path is the store's own tests, not yet seen in the app |
| Rescan library (Jellyfin scheduled task) | 3 | 🟨 | library done; UI pending. `refresh_library` starts the RefreshLibrary scheduled task (not `/Library/Refresh`, which holds the request for the whole scan); `refresh_item` for one item. Administrators only |

## Screens

| Feature | Round | Status | Notes |
|---|---|---|---|
| Home: resume grid (last 6 contexts, "Pick up from {device}") | 2 | 🟨 | Library done; UI pending. `RecentContexts` (store) keeps 24 per server — album, Jellyfin playlist, Aoide playlist, mix, station — saved as JSON; `resume_tiles` gives six, led by "Pick up from {device}" only when that device played more recently than anything here (server age against this clock; a tie goes to this device) |
| Home: shelves (most played, recently added/played/released, random albums, genres) | 1 | ✅ | Recently added, recently played and most played (the last two as the albums of the tracks), recently released, random albums, and a random dozen genres as chips. A shelf with nothing on it is left out. Seen in the app against the stand-in server |
| Home: Discover tile (`/aoide/discover`) | 2 | ⬜ | New: the Electron app has no Discover |
| Albums list and album detail (with finish rate) | 1 | 🟨 | Grid loaded a page at a time, sortable (title, artist, recently added, release date); detail with cover, artist, year, Play, Shuffle, tracks in disc and track order with "Disc 1", "Disc 2" headings on a multi-disc album (seen against the stand-in server). Finish rate not yet |
| Album artists, artists, artist detail (discography, top songs, finish rate) | 1 | 🟨 | Album artists as a grid; an artist's albums. Top songs and finish rate not yet |
| Genres list and detail | 1 | ✅ | A genre's albums through `ListQuery::genre_ids` (Jellyfin `genreIds`). Seen in the app against the stand-in server |
| Songs list | 1 | ✅ | |
| Favourites | 2 | 🟨 | library done; UI pending. `ListQuery::favorites_only` on songs, albums, artists |
| Collections (saved filters) | 3 | 🟨 | library done; UI pending. `query_filters` lists genres, years and tags under a library; a saved filter is a `ListQuery` (genre_ids, years, tags, favorites_only) the app keeps |
| Search: plain Jellyfin search, songs and albums | 1 | ✅ | Songs, albums and artists as you type, 300 ms debounce |
| Search by description (OpenRouter → filters) | 3 | ⬜ | Model only translates to filters; Jellyfin selects |
| Command palette | 3 | ⬜ | |
| Now Playing column: artwork, controls, Queue/Lyrics tabs | 1 | ✅ | Cover, Queue and Lyrics tabs; the transport is the player bar's |
| Full-screen player | 2 | ⬜ | |
| Player bar: transport, seek, shuffle, repeat, volume, output device | 1 | 🟨 | Transport, seek, shuffle, repeat, volume, Crossfade and Infinity switches. Output device picker is Round 2. Shuffle and repeat buttons not yet clicked through in the app |
| Sleep timer (end of song, end of album, timed) | 2 | ⬜ | |
| Context menus on songs, albums, artists, playlists, queue rows | 1 | ✅ | An action strip under song rows and album/playlist headers: Play Next, Play Last, Add to Playlist, Not Interested, Don't Count Plays, Favourite, Go to Album/Artist. Album and artist tiles (their `…`, or a right click) and Aoide playlist rows open the same actions with Play and Shuffle over the window, fetching the songs first. Queue rows have their own buttons. Clicked through against the stand-in server (the right click not tried) |
| Get info | 3 | 🟨 | library done; UI pending. `item_details`: path, container, codec, bitrate, sample rate, bit depth, channels, size, date added, play count |
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
| ReplayGain tags (album preferred) | 2 | 🟨 | Engine done; UI pending: the app calls `Player::set_level_volume` from the Level volume setting. Tags read by the decoder from FLAC/Vorbis comments, ID3v2 `TXXX` and iTunes `----` atoms; album gain first, never above one over the peak. As on the desktop (`loudness.ts`), a file with its own ReplayGain plays by it and the sidecar's measurement stands down (`loudness::level_gain`). Measured on tagged fixtures in `tests/replaygain.rs` |
| Skip silence (`/aoide/sound-bounds`, ignore < 300 ms) | 1 | 🟨 | From `/aoide/sound-bounds`, fetched with the loudness, late batches included; not yet run against a real sidecar |
| Output device picker | 2 | 🟨 | Engine done; UI pending. `output_devices()` lists cpal's devices (stable `host:device` id, name, which is default); `DeviceChoice` (follow the default, or one device by id then name) is serde data for the config; `Output::start_on` / `Output::set_device` switch while playing and carry on from the heard position. A chosen device that will not open, or goes away, gives way to the default with a `DeviceError`. Switching tested on stand-in devices (`tests/devices.rs`); listing and choosing by id seen on macOS CoreAudio, not yet on Linux. Following the default device: a failed stream reopens on the (new) default and plays on from the heard position, queue intact |
| Equalizer (parametric, presets, preamp) | 3 | 🟨 | Engine done; UI pending. `EngineHandle::set_equalizer`: preamp and up to 10 RBJ peaking/shelf bands, gliding over 15 ms; off or flat is bit-transparent. 14 built-in presets (`eq::built_in_presets`), user presets as serde data (`EqPreset`). Measured in `tests/equalizer.rs` |
| Compressor (threshold, ratio, attack, release, makeup, knee) | 3 | 🟨 | Engine done; UI pending. `EngineHandle::set_compressor`: feed-forward, stereo-linked, quadratic soft knee, attack/release smoothed in dB; off is bit-transparent. Presets Gentle, Moderate, Heavy, Late Night. Static curve and time constants measured in `tests/compressor.rs` |
| Visualizer | 3 | 🟨 | Engine done; UI pending (drawing it). Electron used audioMotion and Butterchurn. The render thread copies each heard frame, mono, into a lock-free ring (`EngineHandle::visualizer_tap`); `Visualizer::read` takes the window ending at the heard frame (output latency allowed for), Hann-windows it, runs a real FFT (realfft) and gives log-spaced bands with eased falls, and a waveform. Band energies, alignment and smoothing measured in `tests/visualizer.rs` |
| Fades on pause and resume | 2 | 🟨 | Engine done; UI pending (nothing to wire: every pause and play fades). 150 ms equal-power fades counted in frames; the pause completes, and the position holds, on the fade's last frame, and a resume carries on from the next. Measured in `tests/fades.rs` |
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
| Replay (month, year, all time; top songs, artists, albums) | 2 | 🟨 | Library done; UI pending. `Store::replay(period, now, calendar)`: plays, hours (one decimal), songs, artists and albums, busiest day as a date, top 10 songs, artists and albums, and the unattributed count; periods open at local midnight on the 1st and on 1 January |

## Discovery

| Feature | Round | Status | Notes |
|---|---|---|---|
| Infinity: `/aoide/next`, local taste ranking as fallback | 1 | 🟨 | Fires once when fewer than 3 are left; factor summary on queue rows. The Instant Mix fallback (404) is seen working in the app; the sidecar path only against test doubles |
| Infinity albums mode | 3 | ⬜ | |
| Track, album and artist radio (Instant Mix) | 2 | 🟨 | library done; UI pending. `instant_mix_from` with a track, album, artist, genre or playlist seed; `similar_items`, `similar_albums`, `similar_artists` |
| Taste flags: Not Interested, Don't Count Plays; "Hidden from mixes" list | 1 | ✅ | Set from the action strip; Hidden from mixes lists them in Settings with Clear |
| Mixes (describe a mood → smart rules via OpenRouter) | 3 | ⬜ | |
| Discover | 2 | ⬜ | |

## Playlists

| Feature | Round | Status | Notes |
|---|---|---|---|
| Jellyfin playlists: create, edit, delete, reorder, add/remove songs | 2 | 🟨 | library done; UI pending. Removing and moving go by entry id (`playlist_entries`); delete checks the id is a playlist first |
| Jellyfin playlist cover upload | 3 | 🟨 | library done; UI pending. `set_playlist_cover` / `delete_playlist_cover`; the server allows it for administrators only |
| Aoide playlists: create, rename, notes, delete, add, remove, move (fractional index) | 1 | ✅ | In the app: create, rename, delete, add songs; Edit shows the notes editor and each entry's Up, Down and Remove. A move is the store's `move_item`, one row written, measured past entries the server no longer has. Clicked through against the stand-in server; syncing the edits to a real sidecar not yet seen |
| Add to Aoide playlist (with search past 8, "New playlist…") | 1 | ✅ | |
| Built-in Rediscover Mix and Heavy Rotation | 2 | 🟨 | Library done; UI pending. `Store::ensure_built_in_playlists` creates each under its fixed id only where no row has ever existed, so a deleted built-in stays deleted; the rules are pinned to the other clients' JSON |
| Smart playlists **evaluated** on open | 2 | 🟨 | Library done; UI pending. `Store::resolve_smart` evaluates in SQL; every field/operator pair is tested accepted-and-evaluated or refused by name, as iOS `SmartRule.check` has it. The Electron app syncs the rules but does not evaluate them on the detail screen; do it right here |
| Covers: image hash → sidecar → artwork item → first track | 2 | 🟨 | Library done; UI pending. `Store::playlist_covers` picks image hash → `artworkItemId` → `sourceJellyfinId` → first track's album; `cover_bytes` (sidecar) draws a hash from the `BlobDirectory` or fetches `GET /aoide/images/{sha}` only when asked at draw time, checks the hash and keeps it. `set_cover_from_file` takes a JPEG, PNG or WebP up to 5 MB (not resized); the engine uploads it before the op naming it. Not yet run against a real sidecar |
| Repair covers | 3 | 🟨 | Library done; UI pending. `Store::repair_covers`: a coverless playlist whose name matches exactly one Jellyfin playlist gets that `artworkItemId`, never `sourceJellyfinId` |
| Playlist import (Spotify link, CSV, Exportify; `/aoide/match`) | 3 | 🟨 | Library done; UI pending. `fetch_spotify_playlist` reads the embed page (first 100 tracks, no account), `parse_playlist_csv` Exportify's headers and the other tools' aliases; `match_playlist` asks `POST /aoide/match` first, else the shared rules over Jellyfin search through the `JellyfinSearch` trait the app implements. Held to the 16-row shared match table. Not yet run against a real sidecar or Spotify |
| Folders screen | 3 | ⬜ | Folders sync but the Electron app never had a screen |
| Sharing a playlist with another user | 3 | 🟨 | Library done; UI pending. `SidecarClient::list_shares`, `share_playlist`, `revoke_share`; `SyncEngine::lost_shares` names playlists shared to this user that a re-read no longer lists, and `Store::drop_playlist_locally` drops one without an op. Not yet run against a real sidecar |

## Sync with the sidecar

| Feature | Round | Status | Notes |
|---|---|---|---|
| Push / pull, cursors, acceptedEntities gating | 1 | 🟨 | |
| Per-field merge for playlists, folders, track flags; row merge elsewhere | 1 | 🟨 | |
| Quarantine of unreadable or refused ops | 1 | 🟨 | |
| Covers uploaded before the op naming them | 2 | 🟨 | The engine uploads from the `BlobDirectory` before pushing; tested over HTTP against a mock server |
| Share-revoked edits held back, shares re-read every 5 min | 3 | 🟨 | Library done; UI pending. A push refused with "…is not shared with you for editing" is held in the log, neither quarantined nor pushed again, and listed by `SyncEngine::blocked_edits` (with a count); syncs re-read `/aoide/shares` at most every 5 minutes and release an edit whose share is back. Held in memory, so a restart asks once more |
| Triggers: launch, focus, at most once a minute; "Sync now" | 1 | 🟨 | Launch and window focus, at most once a minute; Sync Now in Settings starts one at once unless one is running, and Settings says the last outcome in words ("Synced: sent 34 changes."). Seen against the stand-in server's empty sync log; not yet run against a real sidecar |
| Sync status panel | 2 | 🟨 | Library done; UI pending. `SyncEngine::overview` returns `SyncOverview`, plain values: running and phase, last synced (kept across restarts) and last attempt, pushed/pulled/applied, waiting, quarantined (and retryable), held back for server support, held for a revoked share, the failure in the server's own words (a `ProblemDetails` title and detail), and whether the server has no sidecar |
| Handoff: publish this device's queue; pick up another's | 2 | 🟨 | Library done; UI pending. `QueueBroadcast` (store) writes the queue row debounced from every player change, never from a screen; `SyncEngine::publish_queue` pushes just that row. `resumable_queues` reads `GET /aoide/queue` (server clock) and falls back to the op log's rows; `ResumableQueue::pick_up` keeps earlier tracks and starts at the named one. Not yet run against a real sidecar |

## Lyrics

| Feature | Round | Status | Notes |
|---|---|---|---|
| Synced lyrics from Jellyfin, follow along, tap to seek | 1 | ✅ | |
| Lyric timing offset | 1 | ✅ | The Settings slider (±1.5 s); applied to line selection and to click-to-seek |
| Internet lyrics (LRCLIB, NetEase, Genius, SimpMusic), on by default, can be turned off | 2 | 🟨 | library done; UI pending. aoide-lyrics, LRCLIB only: NetEase has no documented API, Genius' API has no lyrics, SimpMusic has no terms (crate docs). The off switch and caching what was found are the app's |
| Lyric translation | 3 | ⬜ | The iOS app does it on-device; needs a local option on Linux |
| Furigana and romaji | 3 | 🟨 | library done; UI pending. Kana→romaji always; furigana and dictionary romaji behind aoide-lyrics' `furigana` feature, off by default (IPADIC adds about 48.6 MB and downloads at build time) |

## Settings shown in round 1

Crossfade, Keep albums whole, Infinity on/off, Skip silence, Level volume,
Lyric offset, Hidden from mixes, account (server, user, Sign out). Everything
else in the Electron settings arrives with its feature above.

## Distribution

| Feature | Round | Status | Notes |
|---|---|---|---|
| pacman repository from GitHub releases (`[aoide]`) | 1 | 🟨 | `packaging/arch/PKGBUILD` (package `aoide-linux`) and `.github/workflows/release-arch.yml`: a `v*` tag builds it in an Arch container into the repository `[aoide-linux]` on the release. Installs beside the Electron `aoide`, so its names differ for now (`/usr/bin/aoide-linux`, desktop id `io.github.Virel0.Aoide.Linux`); what taking over the app id `io.github.Virel0.Aoide` takes is at the top of the PKGBUILD. Not yet tagged or built in CI; only the package step was dry-run (on macOS) |
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
