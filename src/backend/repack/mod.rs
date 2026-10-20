//! 离线提取安装包（repack）：从已安装的文件重建一个可分发的 `.deb`。
//!
//! # 原理与局限
//!
//! dpkg 只记录「装了什么文件」，不保留原始包，因此重打包是把
//! `/var/lib/dpkg/status` 里的控制段落 + `info/<包>.list` 的文件清单
//! 重新组装成 ar 归档。产物**不保证**与发行版原始包逐字节一致：
//! conffiles 的本地修改会被打进去，符号链接指向的相对目标原样保留，
//! 而清单里已经丢失的文件会被跳过并计数上报。
//!
//! # 步骤
//!
//! 1. [`files`] 复制文件树到临时工作目录（保留权限与符号链接）；
//! 2. [`control`] 写 `DEBIAN/control` 并搬运维护脚本；
//! 3. [`checksums`] 生成 `DEBIAN/md5sums`；
//! 4. 调用 `dpkg-deb --build --root-owner-group` 打包。
//!
//! 临时目录无论成功失败都会被清理。

pub mod checksums;
pub mod control;
pub mod files;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};

use crate::backend::fs_layout::FsLayout;
use crate::model::InstalledPackage;
use crate::util::{command_exists, sanitize_file_component};

/// 重打包结果。
#[derive(Clone, Debug)]
pub struct BuildOutcome {
    pub path: PathBuf,
    /// 成功复制的文件数。
    pub copied: usize,
    /// 被跳过的条目数（权限不足、已丢失、特殊文件等）。
    pub skipped: usize,
}

/// 从已安装状态重建 deb 包，产物写入 `out_dir`。
pub fn build_package(
    package: &InstalledPackage,
    layout: &FsLayout,
    out_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<BuildOutcome> {
    if package.files.is_empty() {
        bail!("没有可用的安装文件列表，无法离线提取安装包");
    }
    if !command_exists("dpkg-deb") {
        bail!("未找到 dpkg-deb，无法构建安装包");
    }

    progress("正在准备构建目录…");
    let work = temp_work_dir(&package.metadata.name);
    if work.exists() {
        let _ = fs::remove_dir_all(&work);
    }
    fs::create_dir_all(&work).with_context(|| format!("无法创建构建目录 {}", work.display()))?;

    // 用闭包 + 立即清理代替 defer，保证任何提前返回都不会留下临时目录。
    let result = build_in_work_dir(package, layout, out_dir, &work, cancel, progress);
    let _ = fs::remove_dir_all(&work);
    result
}

fn build_in_work_dir(
    package: &InstalledPackage,
    layout: &FsLayout,
    out_dir: &Path,
    work: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<BuildOutcome> {
    let copied = files::copy_installed_files(&package.files, work, cancel, progress)?;
    if copied.copied_files.is_empty() {
        bail!("未能复制任何文件，无法构建安装包");
    }

    let debian_dir = work.join("DEBIAN");
    control::write_control(&debian_dir, package, layout, copied.data_bytes, progress)?;

    checksums::write_md5sums(&debian_dir, &copied.copied_files, cancel, progress)?;

    let path = run_dpkg_deb(
        work,
        out_dir,
        cancel,
        &package.metadata.name,
        &package.metadata.version,
        &package.metadata.arch,
    )?;
    Ok(BuildOutcome {
        path,
        copied: copied.copied_files.len(),
        skipped: copied.skipped,
    })
}

/// 每个复制这么多项上报一次进度；重打包的文件量可能上千，逐条上报会把事件通道淹掉。
pub(crate) const PROGRESS_EVERY: usize = 100;

/// 临时工作目录。带上包名与进程号，便于排查残留，同时避免并发实例互相覆盖。
fn temp_work_dir(pkg_name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "debdeck-build-{}-{}",
        sanitize_file_component(pkg_name),
        std::process::id()
    ))
}

/// 调用 `dpkg-deb --build` 生成产物。
fn run_dpkg_deb(
    work: &Path,
    out_dir: &Path,
    cancel: &AtomicBool,
    name: &str,
    version: &str,
    arch: &str,
) -> Result<PathBuf> {
    // `dpkg-deb` 是子进程，中途终止会留下半个产物，因此只在启动前检查取消。
    ensure_not_cancelled(cancel)?;

    fs::create_dir_all(out_dir)
        .with_context(|| format!("无法创建输出目录 {}", out_dir.display()))?;
    let file_name = format!("{name}_{version}_{arch}_repack.deb");
    let output = out_dir.join(sanitize_file_component(&file_name));
    let _ = fs::remove_file(&output);

    let result = Command::new("dpkg-deb")
        .arg("--root-owner-group")
        .arg("--build")
        .arg(work)
        .arg(&output)
        .output()
        .context("无法执行 dpkg-deb")?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        bail!("dpkg-deb 打包失败：\n{}", stderr.trim());
    }
    Ok(output)
}

/// 设置权限位（deb 对 `DEBIAN/control` 等有明确要求）。
pub(crate) fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("无法设置 {} 的权限", path.display()))
}

/// 供各阶段使用的取消检查。
pub(crate) fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("打包已取消");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::dpkg;

    /// 真机冒烟：挑一个文件数适中的已安装包做重打包。
    ///
    /// 依赖本机 dpkg 数据库与 `dpkg-deb`，因此默认忽略；`cargo test --lib -- --ignored` 会跑它。
    #[test]
    #[ignore = "需要真实 dpkg 数据库与 dpkg-deb"]
    fn repacks_an_installed_package() {
        let layout = FsLayout::system();
        let installed = dpkg::load_installed(&layout, &mut |_| {}).expect("读取 dpkg 数据库");
        let package = installed
            .packages
            .values()
            .find(|package| (5..500).contains(&package.files.len()))
            .expect("应有一个文件数适中的已安装包");

        let out_dir = std::env::temp_dir().join(format!("debdeck-repack-{}", std::process::id()));
        let cancel = AtomicBool::new(false);
        let outcome =
            build_package(package, &layout, &out_dir, &cancel, &mut |_| {}).expect("重打包应成功");
        assert!(outcome.path.is_file());

        // dpkg-deb 自己认得的包才算合格产物。
        let info = Command::new("dpkg-deb")
            .arg("--info")
            .arg(&outcome.path)
            .output()
            .unwrap();
        assert!(
            info.status.success(),
            "dpkg-deb --info 失败：{}",
            String::from_utf8_lossy(&info.stderr)
        );
        let _ = fs::remove_dir_all(&out_dir);
    }
}
