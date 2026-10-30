# Security

## Reporting a vulnerability

Please report security problems privately, through GitHub's private
vulnerability reporting: open the repository's **Security** tab and choose
**Report a vulnerability**, or go straight to
<https://github.com/Virel0/Aoide-Linux/security/advisories/new>. Don't open a
public issue for them.

Say what you found, the release (or the commit, for a source build), how it
was installed (pacman, Flatpak, AppImage, deb or from source), and the steps
that show it. Replies come in the advisory thread; the aim is to release a fix
before the advisory is published.

Problems in the Aoide sidecar plugin belong in
[Virel0/aoide-sidecar](https://github.com/Virel0/aoide-sidecar), and problems
in the iPhone app in [Virel0/Aoide](https://github.com/Virel0/Aoide).

## Supported versions

Only the latest release gets fixes. The app is at 0.x: there are no
long-term branches.

## What the app stores, and where

| What | Where |
|---|---|
| The Melete sign-in token | The system keyring: the Secret Service (GNOME Keyring, KWallet, KeePassXC), or the Secret portal inside the Flatpak |
| The OpenRouter and LibreTranslate keys, when set | The system keyring, write-only: the app never shows or logs them |
| Settings, the server address, the username, the device id | `~/.config/aoide/config.json` |
| Play history, Aoide playlists, taste flags, the sync log | `~/.local/share/aoide/aoide.sqlite` |
| Artwork, LRCLIB answers (30 days) | `~/.cache/aoide` |

Settings → System → Export writes the settings to a file, never the account,
the server, the device id, the token or a key. Signing out removes the token.

## What the app sends, and to whom

The README's [Privacy](README.md#privacy) section lists every service the app
talks to, what it sends and when. In short: your Melete server always; LRCLIB and GitHub's release API by default, both with a switch
to turn them off; LibreTranslate, OpenRouter, Discord and Spotify only when you
set them up or use the feature.

## Hardening in the code

- Redirects are followed only within the server's own scheme, host and port,
  so a redirect never carries the token to another origin. The LibreTranslate
  key and the lyrics are held to their server the same way, and a changed
  translation address forgets the old key.
- Answers are capped: 64 MiB from Melete, 8 MB per artwork
  image, 4 MiB from LibreTranslate, 4 GiB for a stream's temporary file.
- With `XDG_RUNTIME_DIR` set, only that directory is searched for Discord's
  socket, and the socket must be owned by the user running the app.
- Log output from other libraries is held to warnings, since their debug lines
  can carry a stream URL and its token.
- `unsafe` code is forbidden in every crate of the workspace.
- Release builds run in GitHub Actions; the jobs that compile never see the
  signing key ([docs/releases.md](docs/releases.md)).
