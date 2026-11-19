# DebDeck 打包指南

[English](./README.md) · [简体中文](./README_zh-CN.md)

DebDeck 只发布一种原生产物：Ubuntu `.deb` 安装包。打包完全由 Cargo 元数据与
[`cargo-deb`](https://crates.io/crates/cargo-deb) 驱动；仓库刻意不提供额外的
打包包装脚本。

## 单一事实来源

[`Cargo.toml`](../Cargo.toml) 统一定义版本、Debian control 字段、运行时依赖、
维护者脚本、changelog 与安装文件布局。相关输入如下：

| 路径 | 用途 |
| :--- | :--- |
| `Cargo.toml` → `[package]` | crate 版本与通用包元数据 |
| `Cargo.toml` → `[package.metadata.deb]` | Debian 包名、描述、依赖、资产与脚本 |
| `debian/changelog` | Debian 发行历史，安装为 `changelog.Debian.gz` |
| `debian/postinst`、`debian/postrm` | 可选的桌面条目与图标缓存刷新 |
| `packaging/debdeck.desktop` | 应用启动器与 `.deb` 文件关联 |
| `packaging/49-debdeck.rules` | 仅覆盖 `apt-get` / `apt-mark` 的窄范围 polkit 规则 |
| `packaging/icons/` | 安装到 hicolor 主题的多尺寸 PNG 图标 |
| `assets/logo/debdeck-logo.svg` | 可缩放应用图标 |

不要在 shell 脚本里重复依赖清单、安装路径或版本号。属于安装包的数据应写入
Cargo 元数据。

## 本地构建

安装一次 cargo-deb，随后直接调用：

```bash
cargo install cargo-deb --locked
cargo deb --locked --fast --compress-type xz
```

安装包写入 `target/debian/debdeck_<version>_<architecture>.deb`。`cargo deb` 会自行
执行 release 构建。如果已经为同一 target 构建好 release 二进制，可以显式跳过
第二次构建：

```bash
cargo build --locked --release
cargo deb --locked --no-build --fast --compress-type xz
```

包版本来自 `[package].version`；Debian 修订号被刻意留空，因此产物版本是 `0.2.0`
而不是 `0.2.0-1`。

发布构建采用体积优先配置：`opt-level=z`、fat LTO、单 codegen unit、符号裁剪，
并保留默认 unwind panic 策略，确保后台线程 panic 隔离继续有效。安装包使用
xz 快速压缩，以降低安装解包延迟。

## 依赖策略

`depends` 将显式运行时依赖与 `$auto` 合并。cargo-deb 从构建出的 ELF 二进制推导
动态库依赖，使包与 Ubuntu runner 及当前 GPUI/wgpu 系统库集合保持一致，而不是
维护一份容易过期的 `libxcb-*` 手写清单。

图形栈升级后，应检查实际结果，而不是手工修改自动推导的库：

```bash
dpkg-deb -f target/debian/debdeck_*.deb Depends
ldd target/release/debdeck | grep -v 'linux-vdso\|ld-linux'
```

## 校验

CI 流水线同时检查 control 元数据与必须安装的路径。本地可以运行同样的检查：

```bash
DEB="$(find target/debian -maxdepth 1 -name 'debdeck_*.deb' -print -quit)"
dpkg-deb -I "$DEB"
dpkg-deb -c "$DEB"
desktop-file-validate packaging/debdeck.desktop
```

包内至少应包含：

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

在一次性 Ubuntu 机器上进行桌面冒烟测试：

```bash
sudo apt install ./target/debian/debdeck_*.deb
debdeck
```

## CI 与发布流水线

[`ci.yml`](../.github/workflows/ci.yml) 在推送到 `main`、Pull Request 与手动触发时
运行，执行格式检查、零警告 Clippy、测试与 `.deb` 构建，并把安装包作为工作流
产物上传。

[`release.yml`](../.github/workflows/release.yml) 由 `v*` 标签或手动指定的既有标签
触发。它在原生 amd64 与 arm64 Ubuntu runner 上构建，使依赖推导发生在目标架构
本机。产物名为：

```text
debdeck_<version>_linux_amd64.deb
debdeck_<version>_linux_arm64.deb
SHA256SUMS
```

两条工作流都直接调用 `cargo build` 与 `cargo deb`。只有 `vX.Y.Z` 与
`[package].version` 一致时才会接受该标签。

## 发布检查清单

1. 更新 `[package].version` 与 `Cargo.lock`。
2. 在 `debian/changelog` 顶部添加对应记录。
3. 行为变化时同步更新发布相关文档。
4. 运行格式检查、Clippy、测试与本地 `cargo deb --locked --fast --compress-type xz` 构建。
5. 提交并推送 `main`。
6. 创建并推送 annotated `vX.Y.Z` 标签。
7. 确认发布流水线已上传两个 `.deb` 与 `SHA256SUMS`。

## 当前限制

- 安装包通过 GitHub Releases 分发，不进入 APT 仓库。
- 发布包未使用 Ubuntu 归档密钥进行密码学签名。
- 构建目标为 Ubuntu 24.04 或更新版本；其他 Debian 系发行版不在当前打包支持范围。
