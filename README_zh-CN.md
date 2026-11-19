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

  <p>基于 Rust 与 GPUI 构建的 Ubuntu 原生 APT/dpkg 应用管理器。</p>
</div>

---

**DebDeck** 把系统里已安装的软件包和 APT 软件源中的候选包汇总到同一个桌面
目录中。它面向希望完成以下操作、但不想手工拼装 `apt`、`dpkg` 与 `pkexec`
命令的普通 Ubuntu 用户：查找应用、查看应用包含的文件、启动或卸载应用、下载
`.deb`、以及在没有网络时重建安装包。

界面由 [GPUI](https://github.com/zed-industries/zed) 经 `gpui-kit` 原生渲染，
不内嵌浏览器或 WebView。系统变更仍然走 Ubuntu 原有的 `apt-get`、`apt-mark`
与 `dpkg` 路径，授权由 polkit 管理。

## 功能

- **统一软件目录。** 合并已安装的 dpkg 状态与 APT 软件源索引，并保留同一包的
  多个可用版本。
- **快速查找。** 支持应用名、包名和拼音搜索；可只显示图形界面应用，并按名称、
  更新时间或版本排序。
- **完整的包信息。** 展示体积、版本、状态、下载地址、主页、桌面条目、启动命令，
  以及已安装包拥有的文件清单。
- **面向应用的操作。** 通过 `.desktop` 条目启动已安装的图形应用、卸载软件包、
  保持或取消保持版本；也能只为当前用户隐藏启动项，不修改系统级桌面条目。
- **下载指定 APT 产物。** 选择任一可用版本，把对应 `.deb` 下载到下载目录，并显示
  进度、支持取消。
- **离线重建。** 从已安装到磁盘上的文件重新打包，用于原安装包不可用时的备份或
  迁移。DebDeck 会重新计算 control 元数据、安装体积与校验和，而不是直接复制过期
  的包内部数据。
- **本地 `.deb` 安装器。** 在 DebDeck 中打开 `.deb` 后会先检查内容，再通过应用内
  授权流程安装；安装前会明确提示维护脚本将以管理员身份执行。
- **原生渲染与有界任务。** 界面运行在 GPUI 上，软件包扫描与耗时操作由后台任务
  执行，结果通过模态进度页反馈。

应用内界面目前为简体中文。仓库文档以英文为主，并提供
[English README](./README.md)。

## 安装

从 [GitHub Releases](https://github.com/mangerle/debdeck/releases/latest) 下载适合
当前架构的 `.deb`，再通过 APT 安装，以便自动解析依赖：

```bash
sudo apt install ./debdeck_<版本>_linux_amd64.deb
```

ARM64 请把文件名中的 `amd64` 替换为 `arm64`。卸载命令：

```bash
sudo apt remove debdeck
```

发布包面向 Ubuntu；源码构建与打包目前支持 Ubuntu 24.04 或更新版本。

## 从源码构建

依赖：

- Rust **1.95 或更新版本**与 Cargo（edition 2024）
- 可用的 Ubuntu 桌面会话，以及 Vulkan 或 EGL 图形栈
- GPUI 所需的原生开发库

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

开发构建无需安装。Release 构建采用体积优先配置：`opt-level=z`、fat LTO、
单 codegen unit、符号裁剪，并保留默认 unwind panic 策略，确保后台线程 panic
隔离继续有效。

## 构建 Debian 安装包

打包元数据集中在 [`Cargo.toml`](./Cargo.toml) 的 `[package.metadata.deb]`。
仓库刻意**不提供 `build-deb.sh` 或任何本地打包包装脚本**：Cargo 与 cargo-deb
就是打包接口。

```bash
cargo install cargo-deb --locked
cargo deb --locked --fast --compress-type xz
```

产物位于：

```text
target/debian/debdeck_<version>_<architecture>.deb
```

只有在已为选定 target 构建 release 二进制时，才使用
`cargo deb --locked --no-build --fast --compress-type xz`。完整的打包与发布流程见
[`packaging/README_zh-CN.md`](./packaging/README_zh-CN.md)。

## 发布流水线

两条 GitHub Actions 工作流共同保证打包可复现：

| 工作流 | 触发条件 | 用途 |
| :--- | :--- | :--- |
| [`ci.yml`](./.github/workflows/ci.yml) | 推送到 `main`、Pull Request、手动触发 | 执行格式检查、Clippy、测试与 release 构建，并上传校验过的 `.deb` 产物 |
| [`release.yml`](./.github/workflows/release.yml) | 推送 `v*` 标签或手动指定标签 | 构建 amd64 与 arm64 `.deb`，生成 `SHA256SUMS`，并发布 GitHub Release |

发布标签必须为 `vX.Y.Z`，并与 `Cargo.toml` 中的 `version` 一致。所有阶段都不执行
shell 打包脚本；工作流直接调用 `cargo build` 与 `cargo deb`。

## 工程结构

```text
src/model.rs            软件包、软件源、桌面条目与目录数据类型
src/deb822.rs           Debian control/822 解析器
src/backend/            系统边界：APT、dpkg、下载、重打包与桌面集成
src/runtime/            后台任务、监视器、有界事件投递与任务账本
src/app/                应用状态、动作、目录编排与程序入口
src/ui/                 GPUI 视图、详情页、对话框、状态栏与安装窗口
src/util/               路径、版本、格式化、搜索与平台辅助函数
debian/                 Debian changelog 与维护者脚本
packaging/              桌面条目、polkit 规则与 hicolor 图标
.github/workflows/      CI 与标签发布流水线
```

依赖方向为 `ui -> app -> runtime -> backend -> model/deb822/util`。只有 backend 层
读取系统文件、启动子进程或发起网络请求；`ui` 只绘制状态并产生动作。

## 开发检查

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo check --locked --all-targets
```

提交 Pull Request 前请运行上述命令。安全敏感问题请私下报告，不要创建公开 Issue。

## 参与贡献

欢迎提交 Issue 与 Pull Request。较大的改动请先开 Issue 对齐范围。提交信息建议
遵循 Conventional Commits。修改包元数据、文件布局、维护者脚本或发布自动化之前，
请先阅读[打包指南](./packaging/README_zh-CN.md)。

## 开源协议

DebDeck 使用 **Apache License, Version 2.0** 授权，详见 [`LICENSE`](./LICENSE)。
第三方依赖保留各自原始协议，主要包括 GPUI（MIT/Apache-2.0）与 gpui-kit
（Apache-2.0）。
