# Blossom

Org mode note taking for the desktop, modelled on the Android app Bloom.

Blossom works on a folder of `.org` files (a "vault") and edits them in place,
so Emacs, git and Syncthing keep working on the same files. It comes as two
programs:

- `blossom`, the desktop app: outline, reading view, section editor, agenda,
  capture, refile and archive, inline images and diagrams.
- `blsm`, the command-line tool: search, agenda, capture and todo lists.

## Install

Download a file for your system from the
[releases page](https://github.com/kvantos/blossom/releases).

| System | File | How to install |
| --- | --- | --- |
| Debian 12+, Ubuntu 22.04+ | `blossom_VERSION_amd64.deb` | `sudo apt install ./blossom_VERSION_amd64.deb` |
| Arch Linux | `blossom-VERSION-1-x86_64.pkg.tar.zst` | `sudo pacman -U ./blossom-VERSION-1-x86_64.pkg.tar.zst` |
| Any Linux, no install | `Blossom-VERSION-x86_64.AppImage` | `chmod +x Blossom-*.AppImage` and run it (the app only, without `blsm`) |
| Any Linux | `blossom-VERSION-linux-x86_64.tar.gz` | `sudo tar -xzf blossom-VERSION-linux-x86_64.tar.gz -C /usr/local --strip-components=1` |
| macOS, Apple Silicon | `Blossom-VERSION-macos-arm64.dmg` | Open it and drag Blossom to Applications |
| macOS, Intel | `Blossom-VERSION-macos-x86_64.dmg` | Open it and drag Blossom to Applications |

`SHA256SUMS` on the release page lets you check a download:
`sha256sum -c SHA256SUMS --ignore-missing`.

On macOS the app isn't signed yet, so macOS blocks it the first time:

- macOS 15 (Sequoia) and newer: open Blossom once and close the warning, then go
  to System Settings → Privacy & Security, scroll down to the message about
  Blossom and click Open Anyway.
- macOS 14 and older: right-click Blossom in Applications, choose Open, then
  Open again.

After that it opens normally. The `.dmg` also holds `blsm`; copy
it to a folder on your `PATH`, for example `sudo cp /Volumes/Blossom*/blsm /usr/local/bin/`.

## Build from source

You need Rust 1.99 or newer (from your distribution or [rustup](https://rustup.rs)),
a C compiler and `make`.

```sh
git clone https://github.com/kvantos/blossom.git
cd blossom
make            # builds both programs (release mode)
make install    # installs them for you, under ~/.local
```

`make install` puts `blossom` and `blsm` in `~/.local/bin`, plus a desktop entry
and icons so Blossom appears in your app launcher. No `sudo` is needed. If
`~/.local/bin` isn't on your `PATH`, it tells you; add it to use `blsm` by name.
`make install` doesn't build anything, so run `make` first.

- For everyone on the machine: `sudo make install` installs under `/usr/local`.
- Somewhere else: `make install PREFIX=/opt/blossom`.
- To remove it: `make uninstall` (or `sudo make uninstall`, the same way you installed).
- To build packages yourself: `make dist` (tarball), `make deb`, `make appimage`
  (needs `appimagetool`), or `make dmg` on macOS. `make help` lists everything.
- To run the checks CI runs: `make check`.

Without `make`, `cargo install --locked --path crates/blossom-app` and
`cargo install --locked --path crates/blsm` install the two programs into
`~/.cargo/bin` (without the desktop entry and icons).

## Getting started

Point Blossom at your folder of org files:

```sh
blossom ~/notes
```

or set it once in `~/.config/blossom/config.toml`:

```toml
vault = "~/notes"
```

Then `blossom` opens it, and `blsm` uses it too:

```sh
blsm search i.todo t.work   # open tasks tagged work
blsm agenda                 # the next 7 days
blsm capture Buy milk       # a new heading in inbox.org
```

### Search by meaning

In the app, turn on **By meaning** on the search screen. It finds headings
about the same thing even when they use other words. The first time, it
offers to download a small English model (134 MB), then reads your notes
once in the background. From the command line, `blsm search --semantic`
does the same:

```sh
blsm search --semantic something to cook tonight
blsm search --semantic t.work how did the budget meeting go   # filters still narrow
```

`blsm` downloads the model without asking. After the first time only new
and changed headings are read, and a search takes a fraction of a second. Everything runs on your computer.
For notes in other languages, or to use a model you already run, set it in
`config.toml`:

```toml
[semantic]
model = "multilingual"        # about 100 languages (488 MB)
# or an OpenAI-compatible server, such as Ollama:
# model = "server"
# server = "http://localhost:11434/v1"
# server_model = "nomic-embed-text"
# api_key_env = "OPENAI_API_KEY"   # the variable that holds the key, if one is needed
```

## Licence

MIT, see [LICENSE](LICENSE).
