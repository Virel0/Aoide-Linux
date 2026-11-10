# Changelog

Notable changes to Aoide for Linux. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). [TRACKER.md](TRACKER.md) has the
state of every feature in more detail.

## [Unreleased]

### Added
- Home's "Jump back in" tiles show what you last played: clicking one song
  on an album's page makes a tile for that song (its name and artist, the
  album's cover), and the tile plays from that song through the rest of the
  album, or the song alone if the album can't be read. Playing the album
  whole makes it the album's tile again; one album never takes two tiles.
- A Home tile's `…`, or a right click on it, offers Play next and Play last
  for an album, a song or an Aoide playlist, beside its `×`.
- A playlist numbers its songs: each one's place in the list, before its
  cover, songs still waiting to arrive included.
- Colour from the artwork, the new default accent (Settings → General →
  From the artwork). An album's, artist's, Aoide playlist's or web
  collection's page is washed in its cover's colour behind its header,
  fading into the page by the songs, with the cover on a soft shadow, the
  Play button and the page's accents in the cover's colour, and Shuffle and
  the other quiet buttons on a veil over the wash. The sidebar, the side
  panel and the player bar take a soft tint of the playing song's cover,
  and the window's accent (the selected item, the seek bar, switches,
  lyrics' current line) follows it. The colours come from the server's
  palette for the album, or from the picture itself; each is moved in its
  own hue until every word on or in it keeps at least 4.5:1. A grey, black
  or white cover, or nothing playing, leaves the usual look.

### Changed
- The violet and the other fixed accents are still there and look as
  before. A config that kept the old default, the violet, moves to From the
  artwork; any other accent stays as chosen. A config saved by this version
  can't be read by an older one, which starts from its defaults.

### Fixed
- A smart playlist's menu on the Playlists screen uses the songs its rules
  match now; it found none and said there were no songs here.

## [0.10.0] - 2026-11-09

### Added
- Artists show their portrait when the server has one (Melete 0.9.0): a
  picture beside their albums, or one from Wikimedia Commons with the
  server's artists' portraits turned on. It is drawn round on artist tiles
  in Artists and Search, and on the artist's page under their name, where a
  Commons picture shows who took it and its licence, each opening its page
  in the browser. Without one, or from an older server, the round
  placeholder stays.

## [0.9.0] - 2026-11-08

### Added
- An artist's page groups their albums as Melete's web does, under Albums,
  Singles & EPs, Live and Compilations, by each album's MusicBrainz release
  type (Melete 0.8.0). An album of no known type counts as a single or EP
  with three songs or fewer. With albums alone, the page looks as before.
- Search's "Ask for it" results show their cover or the artist's portrait
  when the server gives one (Melete 0.8.0, with its search covers or
  artists' portraits turned on). A portrait shows who took it and its
  licence, each opening its page in the browser.
- Collections saved in Melete's web are listed in the sidebar under
  Collections. Each opens its songs (the first 500) with Play, and Shuffle,
  which plays 500 picked at random from the whole collection by the server.
  A collection can be deleted from the server. A server before Melete 0.8.0
  lists none.

### Fixed
- Settings → Account names the whole command an administrator runs for
  someone who lost their authenticator app,
  `melete user reset-password <name> --remove-second-factor`. Without the
  flag, Melete keeps two-factor sign-in on.
- Rediscover Mix and Heavy Rotation sync for every user on a server, not only
  the first to sync them. They were made under the same id for everyone, and
  Melete gives each playlist id one owner, so every other user's were refused
  (Melete#324). Each user's are now their own. On the first sync after the
  update, the old ones are moved over, keeping a name or rules you changed,
  and one you deleted stays deleted.

## [0.8.0] - 2026-11-06

### Added
- Settings → Account manages your Melete account. Set your profile picture
  from a file (one over 5 MB is made smaller first) or from your server's
  gallery, or remove it; the new one shows at once. Change your password,
  which signs out your other devices and API keys. Turn two-factor sign-in
  on with a QR code or a key for your authenticator app, turn it off, or
  reset it for a new phone, and see its latest changes: wrong codes typed at
  sign-in are pointed out, since they mean someone had your password. An
  administrator sees the server's users and can give one a new password,
  shown once to pass on. Passwords, codes and the new secret are never
  logged or kept, and are dropped when you leave the pane.

### Fixed
- Heavy Rotation and Rediscover Mix fill themselves again on every device.
  This app made them anew on a fresh store before its first sync, writing
  each row and its rules at the same millisecond; on the other devices the
  rules lost the tie, and the playlists became plain and empty. The rules are
  now written a millisecond after the row, the built-ins are made only after
  a sync, and a built-in that lost its rules gets them back.
- A mix saved as a smart playlist reaches the other devices smart, not as an
  empty playlist: it's made in one write with its rules.
- A song from a YouTube playlist export that your library has under another
  artist is found instead of reported missing. When a video's title doesn't
  name its artist, the channel stands in for it, and a channel is often a
  label's ("Ubisoft Music"). Such a song is now looked up in the library by
  its title alone, and taken when every song there with exactly that title is
  by one artist; your server then puts your copy in the playlist instead of
  asking for it again.
- Replay's top songs, artists and albums match the iPhone app's. Songs played
  only on another device were counted in the totals but left out of the lists,
  and smart playlists' listening rules missed them, because this computer had
  never looked them up; after each sync it now asks your server for them.

### Changed
- Heavy Rotation and Rediscover Mix aren't listed until their rules match a
  song.

## [0.7.0] - 2026-11-06

### Added
- Importing a YouTube playlist from a CSV export. A file with the video's
  title and channel, and no artist column, is read video by video: the artist
  from "Artist - Song" titles or a YouTube Music "Artist - Topic" channel, else
  the channel less "VEVO"; "(Official Video)", "Lyric Video", "M/V" and what
  follows a " | " are taken off the title, and a music video's length is
  left out, since it runs over the song's. So Melete asks MusicBrainz for the
  song by its own name and artist, not a video's title alone, which matched
  a different song of that name. Lengths in seconds and `h:mm:ss` are read
  too.

## [0.6.1] - 2026-11-05

### Removed
- Settings → General → Motion covers. An album's own motion cover, from its
  folder, always plays.

### Fixed
- A motion cover kept on this computer stops playing in the full player once
  the server no longer has it. Before, a kept copy played for up to a week,
  so motion covers Melete made from still covers went on showing after
  Melete 0.5.0 removed them.

## [0.6.0] - 2026-11-05

### Changed
- The full-screen player sits on the song's cover, blurred and darkened
  across the screen, with a frosted card holding the cover (or its motion
  cover), the song, the seek bar and the controls. Three layouts, which V goes
  through: the card beside the synced lyrics (the line being sung large in the
  middle, its translation or reading under it), the card alone, and a wide
  card with the song large. It replaces the edge-to-edge cover and the
  visualiser.
- Behind the card, a still blur or one of three moving backgrounds drawn
  from the cover's own colours: Silk, Aurora and Ink. B goes through them,
  and they move only while the music plays and the window has focus.
- The controls and the pointer hide after three still seconds while the music
  plays, and come back with a move of the mouse or a key.
- The layout and background chosen are kept for next time.
- The moving backgrounds take their colours from the server's palette for the
  album (Melete 0.5.0), so they match the iPhone app's; with an older server,
  or an album it has none for, they use the colours found in the cover here.
- Covers are fetched at the size they're drawn (Melete 0.5.0): 160 pixels for
  rows, the player bar and the queue, 400 for playlists' covers, 600 for
  tiles, page headers and Now Playing, and 800 for the full player's card. A
  song row no longer downloads the album's whole folder image. While a larger
  size loads, the one already here stands in. An older server sends the whole
  cover, as before.

### Removed
- Settings → General → Motion covers → Ones your server made. Melete 0.5.0
  no longer makes motion covers from still covers; an album's own motion
  cover, from its folder, still plays.

### Fixed
- Waiting songs an earlier build set aside as unreadable come back into their
  playlists on the next sync. 0.4.0 could not read a playlist entry without a
  track, kept it out of the way, and the server does not send it again; it is
  now put back, unless the song has arrived since, which wins.

## [0.5.0] - 2026-11-03

### Added
- Playlists can hold songs that aren't in your library yet. They show in
  their place, greyed, with the artist and where their request stands
  ("Waiting for approval", "Downloading from Soulseek, 40%", or "Not found"
  and why when nothing could be asked for). They can't be played: Play,
  Shuffle and the queue pass over them. Remove takes one out while editing.
  Once the song arrives, the next sync turns it into a normal song in the
  same place. Needs Melete 0.4.0.
- Search's Ask for It can put a song you don't have in any of your own
  playlists: Add to Playlist… asks for it if it isn't asked for already, and
  the song waits in the playlist until it arrives.
- An open playlist reads itself again after a sync brings changes, so a song
  that arrives shows without leaving the page.

### Changed
- Importing a playlist saves all of it. Save sends every song to your
  server, which makes the playlist with the songs you have and asks for the
  rest, which wait in it until they arrive; the playlist then syncs in and
  opens. "Ask your server for the N" is gone: saving does it. A server older
  than Melete 0.4.0 (or one with MusicBrainz off, or that keeps Aoide's
  playlists from your account) gets the old way: the songs found are saved
  and the rest asked for on the Requests page.

## [0.4.0] - 2026-11-02

### Upgrading from 0.3.0
The app now needs GStreamer, for motion covers. The Arch package, the deb and
the Flatpak bring it. H.264 motion covers also need `gst-libav` on Arch or
`gstreamer1.0-libav` on Debian and Ubuntu; without it the still cover shows.
An OpenRouter or LibreTranslate key you saved stays in the keyring until you
remove it: Melete does both now.

### Added
- Profile pictures from Melete: yours beside your name in Settings → Account,
  each person's in a playlist's Share with list, and the others' in a Listen
  Together session once the server sends them. Someone without a picture, or
  a server from before profile pictures, shows the name's first letter in a
  round tile.
- Discord Rich Presence can show the album's cover again: Settings → System →
  Discord → Show Cover links the album's front on the Cover Art Archive by its
  MusicBrainz id, which Discord fetches. Aoide's icon stands in for an album
  MusicBrainz doesn't know, and is the small picture when the playing badge is
  off.
- Smart playlists (Heavy Rotation, Rediscover Mix, mixes and your own) get a
  cover of their own: the covers of the first four different albums they
  hold, two by two, worked out on this computer and following their songs.
  With one to three albums, the first one's cover. A cover you chose still
  wins. Nothing is uploaded or synced.
- The sidebar shows each playlist's cover beside its name, small and
  rounded, from the same picture the Playlists page draws, or the playlist's
  glyph on a quiet square where it has none.
- Requests keeps itself up to date while it's open, from Melete's request
  feed: no need to press Refresh to see a download move along.
- Each request says more: where a download comes from (torrent or Soulseek),
  how the copy to the server is going (size, speed and time left), Melete's
  latest word on it, who approved or declined it and when, and when it last
  changed.
- Import can ask your server for the songs a playlist has that your library
  doesn't. They go to Melete, which matches each on MusicBrainz and asks for
  it.
- Requests lists those playlists with how many songs were asked for, asked
  already or not found, each song with what became of it, and Remove, which
  takes back the songs still waiting for approval.
- Motion covers: an album with a looping video plays it, muted, over its
  cover on the album's page and in the full-screen player, as on the iPhone.
  Settings → General turns them off for this computer, and while your server
  makes them from still covers, Ones your server made hides those on every
  device. The still cover shows wherever a video can't play.

### Changed
- Search by description and Mixes go through your server's AI: Melete sends
  the words to OpenRouter with its own key, so this computer holds no key and
  never contacts OpenRouter. They're offered while the server has AI on, and
  turn on by themselves at the next start once it runs a Melete with music AI.
- Search by description keeps only what your words say: a genre only when
  you name it, years only with a number or a decade, likes only when you ask
  for favourites, and a name only when you typed it. A song name no longer
  turns into a whole genre's worth of songs.
- The full player shows the cover, still or moving, edge to edge across the
  whole window, fading into the song and the controls along the foot.
- Lyrics, their translation and their pronunciation all come from Melete.
  The server looks lyrics up on LRCLIB when it has none of its own, and
  translates through the LibreTranslate it has set up.
- The lyrics panel's Romaji switch is now Pronunciation, and covers Korean and
  Cyrillic too: each line in Latin letters, from Melete, with kanji read in
  context. Furigana are still worked out on this computer.
- The app now needs GStreamer: the Arch package, the deb and the Flatpak
  bring it. Melete's motion covers are H.264, which needs `gst-libav` on Arch
  and `gstreamer1.0-libav` on Debian and Ubuntu; the Flatpak gets it from the
  runtime's codecs extension.

### Removed
- Settings → Lyrics → Find Lyrics on the Internet, and the LibreTranslate
  server and key: Melete does both. A key you saved stays in the keyring until
  you remove it there.
- Settings → Smart search: the OpenRouter key, the model list and the model
  choice. Melete does both features now. A key you saved stays in the keyring
  until you remove it there.

### Fixed
- Repeat now repeats the queue you made. Once you've added songs with Play
  Next or Play Last, repeat loops the song you started from and the songs you
  added, instead of going back through the rest of the album; that stays in the
  queue for when repeat is off. An album with nothing added still repeats whole.

## [0.3.0] - 2026-10-31

### Added
- Asking for music the library doesn't have: under Search's results, look the
  words up on MusicBrainz through Melete and ask for an album, an artist or a
  song. Requests, in the sidebar, lists what you've asked for and how each is
  getting on, with Withdraw for one still waiting.

## [0.2.0] - 2026-10-31

Aoide for Linux now plays from [Melete](https://github.com/Virel0/Melete), the
server that replaces Jellyfin and keeps Aoide's sync itself. Sign in again
after updating; the local library belongs to the old server and is cleared at
sign-in, and playlists, likes and history come back from Melete.

### Added
- Quick Connect: sign in by approving a code in Melete's web UI.
- Two-factor sign-in: an account with it on in Melete 0.2.0 is asked for the
  code from its authenticator app after the password.
- Home's recently played, recently added and most played shelves come from
  Melete's history of every device, not this computer's alone.
- Albums can be favourited, from Favourite on the album's page. Favourites'
  Albums tab lists them, in place of the albums of liked songs. Album likes
  are their own sync record (`album_likes`), shared with Melete and the
  iPhone app; favouriting an album favourites none of its songs.
- Listen Together: start a session with your queue at the top of the queue
  panel, or join one by its code, and every device in it plays in step, each
  streaming for itself. Infinity is off while in a session.

### Changed
- Searching for songs matches a song's own title and artist, so an album's
  name no longer lists every track on it (Melete 0.2.0).
- Everything the app asked of Jellyfin it asks of Melete: the library, search,
  streams, lyrics, likes, rescans and the user list for sharing. The
  `aoide-jellyfin` crate is now `aoide-melete`.
- A stream's link is asked for when the player starts the track, so a long
  queue starts at once.
- Favourites are your liked songs, with their albums and artists.
- Covers are fetched with your sign-in, and kept a day.
- A playlist cover chosen as an album, on this computer or the iPhone, is
  drawn as that album's cover.
- A song in a format this player can't decode (Opus, WavPack, Monkey's Audio)
  is streamed as FLAC instead, so it plays.
- Radio and Infinity's albums mode start from what `/aoide/next` picks after
  the seed song, in place of Jellyfin's Instant Mix and similar albums.

### Removed
- Jellyfin playlists and Repair Covers: Melete's playlists are Aoide's own.
- Find servers on this network, the playback reports Jellyfin needed, and
  Discord's cover image (Melete's covers need a sign-in).
- Collections' tag filter, artist pictures, and Get Info's file path, size,
  date added and play counts: Melete has none of them.

## [0.1.1] - 2026-10-26

No change in what the app does; the code underneath is reorganised to the
project's size limits, and the audio thread no longer has a way to panic.

### Changed
- The app's message handling is split by feature (`app/`), and its long
  screens, widgets, mixes and smart-search code into focused modules: no
  function over 100 lines, `app.rs` from 1,682 lines to 465.
- The store's smart rules, records and playlists, and the audio engine,
  pipeline and output, are split into submodules with the same public API.
- Five `expect`s on the audio thread are replaced by forms that cannot
  panic.
- Every public item of the library crates is documented, with an `# Errors`
  section on fallible functions, and lints keep it so.
- CI reports functions over 100 lines and source files over 800 as
  warnings.
- The repository is `Virel0/Aoide-Linux`; links, the update check and the
  LRCLIB user agent name it so (the old spelling still resolves).
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

[Unreleased]: https://github.com/Virel0/Aoide-Linux/compare/v0.10.0...HEAD
[0.10.0]: https://github.com/Virel0/Aoide-Linux/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/Virel0/Aoide-Linux/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/Virel0/Aoide-Linux/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/Virel0/Aoide-Linux/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/Virel0/Aoide-Linux/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/Virel0/Aoide-Linux/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/Virel0/Aoide-Linux/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Virel0/Aoide-Linux/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Virel0/Aoide-Linux/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Virel0/Aoide-Linux/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Virel0/Aoide-Linux/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Virel0/Aoide-Linux/releases/tag/v0.1.0
