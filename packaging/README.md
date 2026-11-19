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
| `cliff.toml` | Categorized GitHub Release notes generated from Conventional Commits |

Do not duplicate dependency lists, install paths or version numbers in a shell
script. If a field belongs in the package, put it in Cargo metadata.

## Local build

Install cargo-deb once, then invoke it directly:

```bash
cargo install cargo-deb --locked
cargo deb --locked --fast --compress-type xz
```

The package is written to `target/debian/debdeck_<version>_<architecture>.deb`.
`cargo deb` runs the release build itself. If a release binary for the exact target
has already been built, skip the second build explicitly:

```bash
cargo build --locked --release
cargo deb --locked --no-build --fast --compress-type xz
```

The package version comes from `[package].version`; the Debian revision is
intentionally empty, so the output is `0.2.1`, not `0.2.1-1`.

Release builds use a size-first profile: `opt-level=z`, fat LTO, one codegen unit,
symbol stripping, and the default unwind panic strategy so worker panic isolation
remains effective. Packages use xz fast compression to keep unpack latency low.

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

## Self-update and release signing

DebDeck uses [`shipup`](https://crates.io/crates/shipup) for Linux `.deb` installer
self-updates. Clients read `releases/latest/download/latest.json`, verify the
manifest with an embedded Ed25519 public key, verify the package SHA-256, then run
`pkexec dpkg -i` for the installation step.

The release workflow requires the `SHIPUP_PRIVATE_KEY` repository secret. Use an
independent key for DebDeck and keep the private key only on the maintainer machine
and in GitHub Secrets. Generate the keypair once:

```bash
shipup-cli keygen -o ~/.config/debdeck/update-signing
chmod 600 ~/.config/debdeck/update-signing/ed25519.key
```

Embed the Base64 content of `ed25519.pub` in `UPDATE_PUBLIC_KEY` in
`src/update.rs`, then store the single-line private key as a GitHub Secret:

```bash
gh secret set SHIPUP_PRIVATE_KEY < ~/.config/debdeck/update-signing/ed25519.key
```

Key rotation requires a transition release: older clients only trust the public
key compiled into their binary and cannot verify a manifest signed by the new
private key.

The release job uses the `git-cliff --latest` result for both the GitHub Release
body and the update notes in `latest.json`. Commit messages must therefore follow
Conventional Commits.

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
latest.json
SHA256SUMS
```

Both workflows call `cargo build` and `cargo deb` directly. A tag is accepted only
when `vX.Y.Z` matches `[package].version`. Before publishing, the workflow also
requires a non-empty `SHIPUP_PRIVATE_KEY` and verifies both `.deb` signatures with
the public key embedded in the client.

## Release checklist

1. Update `[package].version` and `Cargo.lock`.
2. Add a matching entry at the top of `debian/changelog`.
3. Update release-facing documentation if behavior changed.
4. Run formatting, Clippy, tests and a local `cargo deb --locked --fast --compress-type xz` build.
5. Commit and push `main`.
6. Confirm that `SHIPUP_PRIVATE_KEY` matches the public key in `src/update.rs`.
7. Create and push an annotated `vX.Y.Z` tag.
8. Confirm that the release workflow published both `.deb` files, `latest.json`
   and `SHA256SUMS`, and that the Release body contains categorized changes.

## Current limitations

- Packages are distributed through GitHub Releases, not an APT repository.
- Release packages are not cryptographically signed by an Ubuntu archive key.
- Builds target Ubuntu 24.04 or newer; other Debian-based distributions are not a
  supported packaging target.
