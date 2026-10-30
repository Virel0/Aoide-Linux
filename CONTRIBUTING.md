# Contributing

Bug reports, fixes and features are welcome. For anything larger than a fix,
open an issue first, so the change can be agreed before the work: the app
follows the iPhone app's behaviour, and [TRACKER.md](TRACKER.md) says what is
being ported and what is not.

## Build and run

```sh
cargo run -p aoide-app
```

You need Rust 1.98 or later and the ALSA and D-Bus development headers:

| Distribution | Packages |
|---|---|
| Arch | `alsa-lib dbus pkgconf` |
| Debian, Ubuntu | `libasound2-dev libdbus-1-dev pkg-config` |

The default build includes furigana, which fetches the IPADIC dictionary at
build time (set `LINDERA_BUILD_DICTIONARY_CACHE_DIR` to build offline from a
copy).

## The checks CI runs

`.github/workflows/ci.yml` runs these on every pull request, on Ubuntu 24.04.
Run them before you push:

```sh
# No aws-lc-rs: this must fail ("did not match any packages")
cargo tree --workspace -i aws-lc-rs --target all

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# MPRIS, single instance and sleep inhibit, on a private D-Bus (Linux)
cargo test -p aoide-mpris -- --include-ignored

# The keyring, against GNOME Keyring on a private D-Bus (Linux)
dbus-run-session -- sh -c 'printf ci | gnome-keyring-daemon --unlock --components=secrets > /dev/null && cargo test -p aoide-app -- --ignored keyring'

# The desktop entry and the AppStream data
desktop-file-validate packaging/arch/io.github.Virel0.Aoide.Linux.desktop
appstreamcli validate --no-net packaging/flatpak/io.github.Virel0.Aoide.Linux.metainfo.xml
```

Clippy warnings fail the build, and `unsafe` code is forbidden in every crate.
CI also reports functions over 100 lines and source files over 800 as
warnings, and Dependabot (`.github/dependabot.yml`) opens weekly grouped
updates for the crates and the pinned actions.

## Making a change

- Keep a pull request to one change. Tests come with the code they cover.
- When a feature's state changes, update its row in [TRACKER.md](TRACKER.md),
  including how it was checked (a test, the app against a stand-in server, a
  real Melete, a Linux desktop).
- Interface text is in sentence case, buttons say what they do, and errors say
  what happened and what to do next.
- Screenshots and test data use made-up artists, albums and accounts: never a
  real server address, library, token or name.

## Commit messages

Look at `git log` for the style. The subject is one plain sentence in the
imperative, sentence case, no prefix and no full stop, saying what the change
does for the listener or the code:

```
Keep the token on the server it was given to
```

The body, wrapped at about 72 columns, says why, what was measured or tested,
and anything left undone.

## Releases

A `v*` tag builds the pacman repository, the AppImage and the deb, and
attaches them to the release; the Flatpak workflow is run by hand with the tag.
Add a `<release>` entry to the AppStream data and a section to
[CHANGELOG.md](CHANGELOG.md) before tagging. Signing is described in
[docs/releases.md](docs/releases.md).

## Licence

By contributing you agree that your work is released under the project's
licence, GPL-3.0-or-later.
