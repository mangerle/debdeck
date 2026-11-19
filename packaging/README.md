# DebDeck Packaging Guide

[English](./README.md) · [简体中文](./README_zh-CN.md)

DebDeck ships one native artifact: an Ubuntu `.deb` package. Packaging is driven
entirely by Cargo metadata and [`cargo-deb`](https://crates.io/crates/cargo-deb);
there is deliberately no wrapper build script in the repository.

## Single source of truth

[`Cargo.toml`](../Cargo.toml) owns the package version, Debian control fields,
runtime dependencies, maintainer scripts, changelog and installed file layout.
The relevant inputs are:

| Path | Purpose |
| :--- | :--- |
| `Cargo.toml` → `[package]` | Crate version and shared package metadata |
| `Cargo.toml` → `[package.metadata.deb]` | Debian package name, description, dependencies, assets and scripts |
| `debian/changelog` | Debian release history installed as `changelog.Debian.gz` |
| `debian/postinst`, `debian/postrm` | Optional desktop and icon cache refreshes |
| `packaging/debdeck.desktop` | Application launcher and `.deb` file association |
| `packaging/49-debdeck.rules` | Narrow polkit rule for `apt-get` / `apt-mark` authorization reuse |
| `packaging/icons/` | Sized PNG icons installed into the hicolor theme |
| `assets/logo/debdeck-logo.svg` | Scalable application icon |

Do not duplicate dependency lists, install paths or version numbers in a shell
script. If a field belongs in the package, put it in Cargo metadata.

## Local build

Install cargo-deb once, then invoke it directly:

```bash
cargo install cargo-deb --locked
cargo deb --locked
```

The package is written to `target/debian/debdeck_<version>_<architecture>.deb`.
`cargo deb` runs the release build itself. If a release binary for the exact target
has already been built, skip the second build explicitly:

```bash
cargo build --locked --release
cargo deb --locked --no-build
```

The package version comes from `[package].version`; the Debian revision is
intentionally empty, so the output is `0.2.0`, not `0.2.0-1`.

## Dependency policy

`depends` combines explicit runtime requirements with `$auto`. cargo-deb derives
the dynamic library dependencies from the built ELF binary. This keeps the package
aligned with the Ubuntu runner and the current GPUI/wgpu system-library set instead
of maintaining a stale hand-written `libxcb-*` list.

After a graphics-stack upgrade, inspect the result rather than editing generated
libraries by hand:

```bash
dpkg-deb -f target/debian/debdeck_*.deb Depends
ldd target/release/debdeck | grep -v 'linux-vdso\|ld-linux'
```

## Validation

The CI pipeline checks both control metadata and required installed paths. The same
checks can be run locally:

```bash
DEB="$(find target/debian -maxdepth 1 -name 'debdeck_*.deb' -print -quit)"
dpkg-deb -I "$DEB"
dpkg-deb -c "$DEB"
desktop-file-validate packaging/debdeck.desktop
```

The package must contain at least:

```text
usr/bin/debdeck
usr/share/applications/debdeck.desktop
usr/share/icons/hicolor/scalable/apps/debdeck.svg
usr/share/pixmaps/debdeck.png
usr/share/polkit-1/rules.d/49-debdeck.rules
usr/share/doc/debdeck/copyright
usr/share/doc/debdeck/changelog.Debian.gz
usr/share/doc/debdeck/README.md
```

For a desktop smoke test on a disposable Ubuntu machine:

```bash
sudo apt install ./target/debian/debdeck_*.deb
debdeck
```

## CI and release pipeline

[`ci.yml`](../.github/workflows/ci.yml) runs on pushes to `main`, pull requests and
manual dispatch. It enforces formatting, Clippy with warnings denied, tests, and a
successful `.deb` build, then uploads the package as a workflow artifact.

[`release.yml`](../.github/workflows/release.yml) runs for `v*` tags or a manually
provided existing tag. It builds on native amd64 and arm64 Ubuntu runners so
dependency discovery is performed on the target architecture. Artifacts are named:

```text
debdeck_<version>_linux_amd64.deb
debdeck_<version>_linux_arm64.deb
SHA256SUMS
```

Both workflows call `cargo build` and `cargo deb` directly. A tag is accepted only
when `vX.Y.Z` matches `[package].version`.

## Release checklist

1. Update `[package].version` and `Cargo.lock`.
2. Add a matching entry at the top of `debian/changelog`.
3. Update release-facing documentation if behavior changed.
4. Run formatting, Clippy, tests and a local `cargo deb --locked` build.
5. Commit and push `main`.
6. Create and push an annotated `vX.Y.Z` tag.
7. Confirm that the release workflow published both `.deb` files and `SHA256SUMS`.

## Current limitations

- Packages are distributed through GitHub Releases, not an APT repository.
- Release packages are not cryptographically signed by an Ubuntu archive key.
- Builds target Ubuntu 24.04 or newer; other Debian-based distributions are not a
  supported packaging target.
