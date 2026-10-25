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
| Home: resume grid (last 6 contexts, "Pick up from {device}") | 2 | ✅ | "Jump Back In" on Home: `resume_tiles` over `RecentContexts` (kept per server in `recent-contexts.json` beside the store), led by "Pick up from {device}" only when that device played more recently than anything here. Contexts are remembered when playback replaces the queue from an album's or an Aoide playlist's own page (Play, Shuffle, a row) or from an album or playlist menu's Play/Shuffle; a tile plays at once, its `×` removes it. Picking up plays the other device's list from its track and seeks to its place; that queue is not offered again until it changes. Seen against the stand-in server (its `/aoide/queue`); Jellyfin playlist and station contexts arrive with their screens. A mix tile says the Mixes screen is missing |
| Home: shelves (most played, recently added/played/released, random albums, genres) | 1 | ✅ | Recently added, recently played and most played (the last two as the albums of the tracks), recently released, random albums, and a random dozen genres as chips. A shelf with nothing on it is left out. Seen in the app against the stand-in server |
| Home: Discover tile (`/aoide/discover`) | 2 | ✅ | New: the Electron app has no Discover. The tile shows the week's count; fetched when Home opens and again only once the UTC ISO week has turned (`iso_week`); a 404 hides it for the session, another failure tries again at the next Home. Seen against the stand-in server, with and without the endpoint |
| Albums list and album detail (with finish rate) | 1 | ✅ | Grid loaded a page at a time, sortable (title, artist, recently added, release date); detail with cover, artist, year, Play, Shuffle, tracks in disc and track order with "Disc 1", "Disc 2" headings on a multi-disc album, and "You finish N% of this album's songs" from the local history (nothing under three decided listens). Seen against the stand-in server |
| Album artists, artists, artist detail (discography, top songs, finish rate) | 1 | ✅ | Album artists as a grid; an artist's page: "You finish N% of this artist's songs" over every song on their albums, Top Songs (ten, by this listener's plays in the history, then the server's play count; unplayed songs left out), then their albums. `ListQuery` has no artist filter, so the songs come album by album. Seen against the stand-in server |
| Genres list and detail | 1 | ✅ | A genre's albums through `ListQuery::genre_ids` (Jellyfin `genreIds`). Seen in the app against the stand-in server |
| Songs list | 1 | ✅ | |
| Favourites | 2 | ✅ | A sidebar section with Songs, Albums and Artists tabs (`ListQuery::favorites_only`; artists without a library parent), Play and Shuffle over the songs. Seen against the stand-in server |
| Collections (saved filters) | 3 | ✅ | Sidebar → New Collection…: songs or albums, filtered by any of the library's genres, years and tags (`query_filters`, for the kind shown) and Only favourites, the results updating as they are picked; Save as Collection keeps it by name in the config file and lists it in the sidebar's Collections, where it reopens with Save Changes and Delete (after asking). Seen against the stand-in server |
| Search: plain Jellyfin search, songs and albums | 1 | ✅ | Songs, albums and artists as you type, 300 ms debounce |
| Search by description (OpenRouter → filters) | 3 | 🟨 | Library done; UI pending. `aoide_integrations::openrouter`: `translate_search` (prompt, parsing and checks ported from the Electron app with its test cases; the iOS app's sort directions), model picked by id from `models()` with prices, the key passed by the caller and kept in the keyring through `ApiKeyStore`. Only the phrase and the genre names are sent; the model only translates to filters, Jellyfin selects. Tested against a local stand-in, not yet against OpenRouter |
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
| Tray: play/pause, next, previous, stop, open, quit; minimise/exit to tray, start minimised | 2 | 🟨 | Library done; UI pending. `aoide_mpris::Tray`: StatusNotifierItem and its DBusMenu on zbus (not `ksni`, which only speaks to the session bus), re-registers when a panel starts later or restarts; clicks arrive as `TrayCommand`s, a left click toggles the window, a middle click plays or pauses; `is_shown()` says whether a panel took it. Private-bus tests in `tests/tray.rs` (ignored by default) not yet run on Linux; the Linux code cross-checked with clippy. Minimise/exit to tray and start minimised are the app's |
| Prevent sleep / suspend while playing | 2 | 🟨 | Came with MPRIS: aoide-mpris's inhibitor is set whenever playing starts or stops. Not yet run on a Linux desktop |
| Discord Rich Presence (Aoide's application id, options as in Electron) | 2 | 🟨 | Library done; UI pending. `aoide_integrations::discord::Presence`: Discord's local socket (`discord-ipc-0..9` under `XDG_RUNTIME_DIR`, Flatpak and snap subpaths included), "Listening" with song, artist, album, time bar; display type (app name / artist / song), shown when paused, cover only if a public `https` URL without credentials (else Aoide's `icon` asset), playing/paused badge, links only if free of tracking parameters. Opens no socket while off, at most one update a second, finds a Discord started later. Tested against a stand-in Discord socket; not yet seen in a real Discord client |
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
| Finish rate on album and artist headers | 2 | ✅ | `FinishRate` summed over the album's or the artist's songs; absent under three decided listens. Seen moving from 100% to 70% after three skips, against the stand-in server |
| Replay (month, year, all time; top songs, artists, albums) | 2 | ✅ | A sidebar section: This Month / This Year (the default, as on the phone) / All Time from `Store::replay` on the local calendar; Plays, Hours, Songs and Artists tiles, the busiest day, Top Songs fetched in rank order and playable, Top Artists and Top Albums with play counts, and the unattributed plays named when there are any. Seen against the stand-in server's history; the periods differ only with older history than it had |

## Discovery

| Feature | Round | Status | Notes |
|---|---|---|---|
| Infinity: `/aoide/next`, local taste ranking as fallback | 1 | 🟨 | Fires once when fewer than 3 are left; factor summary on queue rows. The Instant Mix fallback (404) is seen working in the app; the sidecar path only against test doubles |
| Infinity albums mode | 3 | ✅ | Settings → Infinity → Add Whole Albums. When the queue runs short, a pool of ten albums (Jellyfin's similar albums, then the albums of the playing song's Instant Mix, then the artist's, then any, none already queued) is ranked by the Electron app's rule (`chooseAlbumsByTaste`: an album judged on its tracks' genres, summed finish counts, and as just heard if any track was) and the best two are queued whole, in disc and track order, Not Interested songs left out, each row saying "A whole album you might like". Decided: in albums mode the local ranking is used whether or not the sidecar answers `/aoide/next`, which ranks songs, not records; the desktop's albums mode doesn't ask it either. Seen against the stand-in server |
| Track, album and artist radio (Instant Mix) | 2 | ✅ | Start Radio in every song's and album/artist/playlist menu and on album, artist, genre and Jellyfin playlist pages: `instant_mix_from` with that seed (an Aoide playlist's first song seeds it), Not Interested songs left out, played at once and remembered as "{name} Radio" for the resume grid, which seeds an album or artist station the same way again and the rest with the kind-blind Instant Mix. Seen against the stand-in server. `similar_*` not used yet |
| Taste flags: Not Interested, Don't Count Plays; "Hidden from mixes" list | 1 | ✅ | Set from the action strip; Hidden from mixes lists them in Settings with Clear |
| Mixes (describe a mood → smart rules via OpenRouter) | 3 | 🟨 | Library done; UI pending. `describe_mix` returns rules in the phone's `smartRules` JSON (byte for byte, illegal field/operator pairs refused with a reason) and the names the mood mentioned; `mix_plan` has the genre matching ("rock" takes Hard Rock, not Rockabilly), name-first ordering and the summary line, ported from the Electron app. Evaluating the history rules against the local store is the app's (aoide-store). Tested against a local stand-in, not yet against OpenRouter |
| Discover | 2 | ✅ | The screen: the 30 songs in the server's order, Play, Shuffle, any row to start from, as the iOS `DiscoverView`. Seen against the stand-in server |

## Playlists

| Feature | Round | Status | Notes |
|---|---|---|---|
| Jellyfin playlists: create, edit, delete, reorder, add/remove songs | 2 | ✅ | Playlists has an Aoide / Jellyfin switch. The Jellyfin list creates; a playlist's page plays and shuffles (remembered for the resume grid), renames, deletes after asking, and in Edit moves entries up and down and removes them, by entry id. Add to Playlist… offers Aoide and Jellyfin playlists under their own headings. Seen against the stand-in server |
| Jellyfin playlist cover upload | 3 | ✅ | Choose Cover… opens the system's file dialog (rfd: NSOpenPanel on macOS, the XDG portal or zenity on Linux) for a JPEG, PNG or WebP up to 20 MB; Remove Cover. A non-administrator is told "Only an administrator of this Jellyfin server can change a playlist's cover" with the 403. Both seen against the stand-in server on macOS; the Linux dialog not yet run |
| Aoide playlists: create, rename, notes, delete, add, remove, move (fractional index) | 1 | ✅ | In the app: create, rename, delete, add songs; Edit shows the notes editor and each entry's Up, Down and Remove. A move is the store's `move_item`, one row written, measured past entries the server no longer has. Clicked through against the stand-in server; syncing the edits to a real sidecar not yet seen |
| Add to Aoide playlist (with search past 8, "New playlist…") | 1 | ✅ | |
| Built-in Rediscover Mix and Heavy Rotation | 2 | ✅ | `Store::ensure_built_in_playlists` runs once the store is known to be this account's, before the playlists are read; each is created under its fixed id only where no row has ever existed, so a deleted built-in stays deleted. Seen created in the app against the stand-in server |
| Smart playlists **evaluated** on open | 2 | ✅ | A smart playlist's page evaluates its rules against the local history when it opens (`Store::resolve_smart`, local calendar), lists the songs in the rules' order, plays them as `Smart`, and offers no moving or removing; the list says "Smart · N songs" from `match_count_of`. A rule set that won't read says so. Seen with Heavy Rotation against the stand-in server's history |
| Covers: image hash → sidecar → artwork item → first track | 2 | ✅ | The Playlists list and a playlist's page draw `Store::playlist_covers`: a chosen picture from the blob directory (`covers/` beside the store), fetched with `cover_bytes` from `GET /aoide/images/{sha}` only when the page is about to draw it; an item or first track by its image tag. Choose Cover… (the system file dialog) keeps a JPEG, PNG or WebP up to 5 MB with `set_cover_image`; Remove Cover takes it off. The sync engine is given the blob directory and uploads the picture before the change naming it. Seen against the stand-in server: chosen, uploaded on Sync Now, deleted here and fetched back from the stand-in at draw time, removed. Not yet a real sidecar |
| Repair covers | 3 | ✅ | Repair Covers in Settings → Sync → Details: the Jellyfin playlists' names to `Store::repair_covers`, and how many it gave a cover, in words. Seen against the stand-in server |
| Playlist import (Spotify link, CSV, Exportify; `/aoide/match`) | 3 | ✅ | Playlists → Import…: paste a Spotify link or choose a CSV file; matching shows its progress, then what was found (with the library song it picked) and what was not; Save N Songs makes an Aoide playlist and opens it; Copy Missing Songs puts "Title — Artist" lines on the clipboard; the 100-song limit of a link is said. The app's `JellyfinSearch` is song search inside the music library. Seen against the stand-in server, with a development-only `AOIDE_DEV_SPOTIFY_ORIGIN` pointing the link at a stand-in page; not yet real Spotify or a real `/aoide/match` |
| Folders screen | 3 | ⬜ | Folders sync but the Electron app never had a screen |
| Sharing a playlist with another user | 3 | ✅ | A playlist's Share… page: who it is shared with (or who shared it with you), Share with someone else (picked from `GET /Users` when the server lists users for this account, otherwise a typed username looked up among `GET /Users/Public`, or a pasted user id), Stop Sharing / Leave after asking, and the edits held back for it because a share was revoked. The share list is read through `SyncEngine::refresh_shares`, so a share given back lets its held edits go at once; the engine now knows the user's id. Seen against the stand-in server as an administrator and as a plain user. Not done: dropping a playlist whose share to this user was revoked (`lost_shares` → `drop_playlist_locally`); the client keeps no body for a 404 or 403, so those refusals are worded from the contract |

## Sync with the sidecar

| Feature | Round | Status | Notes |
|---|---|---|---|
| Push / pull, cursors, acceptedEntities gating | 1 | 🟨 | |
| Per-field merge for playlists, folders, track flags; row merge elsewhere | 1 | 🟨 | |
| Quarantine of unreadable or refused ops | 1 | 🟨 | |
| Covers uploaded before the op naming them | 2 | 🟨 | The engine uploads from the `BlobDirectory` before pushing; tested over HTTP against a mock server |
| Share-revoked edits held back, shares re-read every 5 min | 3 | 🟨 | Library done; UI pending. A push refused with "…is not shared with you for editing" is held in the log, neither quarantined nor pushed again, and listed by `SyncEngine::blocked_edits` (with a count); syncs re-read `/aoide/shares` at most every 5 minutes and release an edit whose share is back. Held in memory, so a restart asks once more |
| Triggers: launch, focus, at most once a minute; "Sync now" | 1 | 🟨 | Launch and window focus, at most once a minute; Sync Now in Settings starts one at once unless one is running, and Settings says the last outcome in words ("Synced: sent 34 changes."). Seen against the stand-in server's empty sync log; not yet run against a real sidecar |
| Sync status panel | 2 | ✅ | Settings → Sync → Details…: `SyncEngine::overview` in plain words — last synced ("4 minutes ago") or the phase while running (refreshed each second), what the last sync sent and brought in, how many wait, how many are held back and for which kinds ("taste flags"), how many are held for a revoked share and for which playlist, how many were set aside, the failure in the server's words, a missing sidecar. Sync Now, and Retry for the changes the server failed on (`requeue_quarantined`, then a sync). Every case seen against the stand-in server, which refuses, fails and revokes on purpose |
| Handoff: publish this device's queue; pick up another's | 2 | ✅ | Every player change (new queue, skip, reorder, seek, pause, MPRIS commands, the 10 s report) notes the queue into a `QueueBroadcast` on the store thread; a written row is pushed alone by `publish_queue` (a sync asked for meanwhile waits for it), and quitting flushes. Pick-up from Home's grid. Seen against the stand-in server's sidecar endpoints; not yet a real sidecar or a second real device |

## Lyrics

| Feature | Round | Status | Notes |
|---|---|---|---|
| Synced lyrics from Jellyfin, follow along, tap to seek | 1 | ✅ | |
| Lyric timing offset | 1 | ✅ | The Settings slider (±1.5 s); applied to line selection and to click-to-seek |
| Internet lyrics (LRCLIB, NetEase, Genius, SimpMusic), on by default, can be turned off | 2 | 🟨 | library done; UI pending. aoide-lyrics, LRCLIB only: NetEase has no documented API, Genius' API has no lyrics, SimpMusic has no terms (crate docs). The off switch and caching what was found are the app's |
| Lyric translation | 3 | 🟨 | Library done; UI pending. `aoide_integrations::translate`: the lyrics' language by whatlang and the listener's from the locale, offered only when they differ; with the `local-translation` feature (off by default), `LocalTranslator` translates on this machine with Firefox Translations' models through `fxtranslate` (pure Rust, MPL-2.0), each pair's model (about 20 to 40 MB) downloaded on first use to the cache dir, through English when there is no direct model. Lyrics never leave the machine. Not yet run with a real model (the test for it is ignored and needs a downloaded model); fxtranslate 0.5.0 has a decoder fix waiting in its next release, and Mozilla's model hosting is meant for Firefox, so a mirror (`ModelSource::Mirror`) is worth setting up |
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
| In-app "update available" notice | 3 | 🟨 | Library done; UI pending. `aoide_integrations::update::UpdateChecker`: GitHub's latest release of Virel0/aoide-linux against the running version (semver, drafts and pre-releases ignored), with the notes and the release page; at most one attempt a day through an `UpdateState` the app keeps; sends only a fixed `User-Agent: aoide-linux` and GitHub's API headers. The off switch is the app's. pacman and Flatpak update the app themselves. Tested against a local stand-in for GitHub |
| Settings import/export, log level, clear cache | 3 | ⬜ | |

## Not porting

MPV and jukebox; internet radio; folder browser; tag editor; remote-control
server; share links; ratings; Navidrome and Subsonic; external links and
Last.fm; lyric provider picker; hotkey rebinding; theme gallery, fonts, zoom,
backgrounds; the 32 bundled translations (English only, French later on both
apps); playback speed and pitch; Feishin's fixed crossfade; analytics; Windows
and macOS builds.
