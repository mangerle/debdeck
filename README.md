<div align="center">
  <img src="./assets/logo/debdeck-logo.svg" alt="DebDeck" width="320" />

  <p>
    <a href="./README.md"><strong>English</strong></a> ·
    <a href="./README_zh-CN.md"><strong>简体中文</strong></a>
  </p>

  <p>
    <a href="https://github.com/mangerle/debdeck/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-blue" /></a>
    <img alt="Rust" src="https://img.shields.io/badge/rust-1.95%2B-orange" />
    <img alt="Platform" src="https://img.shields.io/badge/platform-Ubuntu-informational" />
    <img alt="Status" src="https://img.shields.io/badge/status-alpha-red" />
  </p>

  <p>A native APT and dpkg application manager for Ubuntu, built with Rust and GPUI.</p>
</div>

---

**DebDeck** puts the packages you have installed and the packages available from
your APT sources into one desktop catalog. It is designed for ordinary Ubuntu
users who need to find an application, inspect what it owns, launch or remove it,
download its `.deb`, or recover a package while offline — without composing
`apt`, `dpkg`, and `pkexec` commands by hand.

The interface is rendered natively with [GPUI](https://github.com/zed-industries/zed)
through `gpui-kit`; there is no embedded browser or web view. System changes still
go through Ubuntu's normal `apt-get`, `apt-mark`, and `dpkg` paths, and privilege
escalation is mediated by polkit.

## Features

- **One package catalog.** Merges installed dpkg state with APT repository
  indexes, including multiple available versions of the same package.
- **Fast discovery.** Search by application name, package name, or Pinyin; filter
  to GUI applications; sort by name, update time, or version.
- **Useful package details.** Shows size, version, status, download URL,
  homepage, desktop entry, launch command, and the files owned by an installed
  package.
- **Application-aware actions.** Open installed GUI applications through their
  `.desktop` entries, remove packages, hold or unhold a version, and hide an
  application launcher for the current user without editing the system entry.
- **Download exact APT artifacts.** Pick an available version and save its `.deb`
  to the downloads directory with progress and cancellation support.
- **Offline rebuild.** Repack an installed package from its on-disk files for
  backup or transfer when the original archive is unavailable. DebDeck recomputes
  control metadata, installed size, and checksums instead of copying stale package
  internals.
- **Local `.deb` installer.** Opening a `.deb` file in DebDeck inspects it first and
  installs it through an in-application authorization flow. Maintainer scripts
  are called out explicitly before installation.
- **Native rendering and bounded work.** The UI runs on GPUI, package scanning and
  long operations run on background tasks, and completion status is surfaced in
  modal progress views.

The current in-application UI is Simplified Chinese. The repository documentation
is maintained in English first, with a [简体中文 README](./README_zh-CN.md).

## Install

Download the `.deb` for your architecture from
[GitHub Releases](https://github.com/mangerle/debdeck/releases/latest), then install
it with APT so dependencies are resolved:

```bash
sudo apt install ./debdeck_<version>_linux_amd64.deb
```

For ARM64, replace `amd64` with `arm64`. Remove the application with:

```bash
sudo apt remove debdeck
```

The release package targets Ubuntu. Building and packaging are currently supported
on Ubuntu 24.04 or newer.

## Build from source

Requirements:

- Rust **1.95 or newer** with Cargo (edition 2024)
- An Ubuntu desktop session with a working Vulkan or EGL graphics stack
- The native development libraries required by GPUI

```bash
git clone https://github.com/mangerle/debdeck.git
cd debdeck

sudo apt install -y \
  pkg-config libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libfontconfig1-dev libfreetype6-dev libdbus-1-dev libxcb1-dev \
  libx11-dev libxi-dev libxcursor-dev libxrandr-dev libxinerama-dev \
  libgl1-mesa-dev

cargo run --release
```

The development binary does not need an install step. Release builds strip
symbols and use thin LTO.

## Create the Debian package

Packaging metadata is defined once in [`Cargo.toml`](./Cargo.toml) under
`[package.metadata.deb]`. There is intentionally **no `build-deb.sh` or other
local packaging wrapper**: Cargo and cargo-deb are the packaging interface.

```bash
cargo install cargo-deb --locked
cargo deb --locked
```

The artifact is written to:

```text
target/debian/debdeck_<version>_<architecture>.deb
```

Use `cargo deb --locked --no-build` only when a release binary has already been
built for the selected target. The complete packaging and release process is
documented in [`packaging/README.md`](./packaging/README.md).

## Release pipeline

Two GitHub Actions workflows keep packaging reproducible:

| Workflow | Trigger | Purpose |
| :--- | :--- | :--- |
| [`ci.yml`](./.github/workflows/ci.yml) | Push to `main`, pull request, manual | Format, Clippy, tests, release build, and a validated `.deb` artifact |
| [`release.yml`](./.github/workflows/release.yml) | Push a `v*` tag or manual tag dispatch | Builds amd64 and arm64 `.deb` files, creates `SHA256SUMS`, and publishes a GitHub Release |

The release tag must be `vX.Y.Z` and must match the `version` field in `Cargo.toml`.
No shell packaging script is executed at any stage; both workflows invoke
`cargo build` and `cargo deb` directly.

## Project layout

```text
src/model.rs            package, repository, desktop-entry, and catalog data types
src/deb822.rs           Debian control/822 parser
src/backend/            system and process boundary: APT, dpkg, downloads, repack, desktop
src/runtime/            background tasks, watchers, bounded event delivery, and ledger
src/app/                application state, actions, catalog orchestration, and entry point
src/ui/                 GPUI views, details, dialogs, status bar, and install window
src/util/               paths, versions, formatting, search, and platform helpers
debian/                 Debian changelog and maintainer scripts
packaging/              desktop entry, polkit rule, and hicolor icons
.github/workflows/      CI and tagged-release pipelines
```

The dependency direction is `ui -> app -> runtime -> backend -> model/deb822/util`.
Only the backend layer reads system files, starts subprocesses, or performs network
requests; `ui` renders state and emits actions.

## Development checks

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo check --locked --all-targets
```

Please run these before opening a pull request. Report security-sensitive issues
privately rather than in a public issue.

## Contributing

Issues and pull requests are welcome. For larger changes, open an issue first to
agree on the scope. Conventional Commit messages are preferred. See the
[Packaging guide](./packaging/README.md) before changing package metadata, file
layouts, maintainer scripts, or release automation.

## License

DebDeck is licensed under the **Apache License, Version 2.0**. See [`LICENSE`](./LICENSE).
Third-party dependencies retain their original licenses, including GPUI
(MIT/Apache-2.0) and gpui-kit (Apache-2.0).
