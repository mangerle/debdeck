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
use std::time::Duration;

use crate::backend::{BackendError, Result};

use crate::backend::dpkg;
use crate::backend::fs_layout::FsLayout;
use crate::deb822::Stanza;
use crate::model::InstalledPackage;
use crate::util::{command_exists, run_command, sanitize_file_component};
use tempfile::Builder as TempDirBuilder;

/// 重打包结果。
#[derive(Clone, Debug)]
pub struct BuildOutcome {
    pub path: PathBuf,
    /// 成功复制的文件数。
    pub copied: usize,
    /// 被跳过的条目数（权限不足、已丢失、特殊文件等）。
    pub skipped: usize,
}

/// 重打包所需的完整输入。
pub struct BuildOptions<'a> {
    /// 已安装软件包记录。
    pub package: &'a InstalledPackage,
    /// 系统路径布局。
    pub layout: &'a FsLayout,
    /// 产物目录。
    pub out_dir: &'a Path,
    /// 取消标志。
    pub cancel: &'a AtomicBool,
    /// 阶段进度回调。
    pub progress: &'a mut dyn FnMut(&str),
}

/// 从已安装状态重建 deb 包，产物写入 `options.out_dir`。
pub fn build_package(mut options: BuildOptions<'_>) -> Result<BuildOutcome> {
    let package = options.package;
    let package_files = dpkg::installed_files(
        options.layout,
        &package.metadata.name,
        &package.metadata.arch,
    )
    .unwrap_or_default();
    if package_files.is_empty() {
        return Err(BackendError::invalid(
            "离线重建安装包",
            "没有可用的安装文件列表，无法离线提取安装包",
        ));
    }
    if !command_exists("dpkg-deb") {
        return Err(BackendError::missing_program("离线重建安装包", "dpkg-deb"));
    }
    let control = dpkg::load_package_control(
        options.layout,
        &package.metadata.name,
        &package.metadata.arch,
    )?
    .unwrap_or_else(Stanza::default);

    (options.progress)("正在准备构建目录…");
    let prefix = format!(
        "debdeck-build-{}-",
        sanitize_file_component(&package.metadata.name)
    );
    let work = TempDirBuilder::new()
        .prefix(&prefix)
        .tempdir()
        .map_err(|source| {
            BackendError::io("创建构建目录", std::env::temp_dir().display(), source)
        })?;

    build_in_work_dir(&mut options, &package_files, &control, work.path())
}

fn build_in_work_dir(
    options: &mut BuildOptions<'_>,
    package_files: &[String],
    control: &Stanza,
    work: &Path,
) -> Result<BuildOutcome> {
    let copied =
        files::copy_installed_files(package_files, work, options.cancel, options.progress)?;
    if copied.copied_files.is_empty() {
        return Err(BackendError::invalid(
            "离线重建安装包",
            "未能复制任何文件，无法构建安装包",
        ));
    }

    let debian_dir = work.join("DEBIAN");
    control::write_control(control::ControlWrite {
        debian_dir: &debian_dir,
        package: options.package,
        control,
        layout: options.layout,
        data_bytes: copied.data_bytes,
        progress: options.progress,
    })?;
    checksums::write_md5sums(
        &debian_dir,
        &copied.copied_files,
        options.cancel,
        options.progress,
    )?;
    let path = run_dpkg_deb(&DpkgBuild {
        work,
        out_dir: options.out_dir,
        cancel: options.cancel,
        name: &options.package.metadata.name,
        version: &options.package.metadata.version,
        arch: &options.package.metadata.arch,
    })?;
    Ok(BuildOutcome {
        path,
        copied: copied.copied_files.len(),
        skipped: copied.skipped,
    })
}

/// 每个复制这么多项上报一次进度；重打包的文件量可能上千，逐条上报会把事件通道淹掉。
pub(crate) const PROGRESS_EVERY: usize = 100;

#[derive(Clone, Copy)]
struct DpkgBuild<'a> {
    work: &'a Path,
    out_dir: &'a Path,
    cancel: &'a AtomicBool,
    name: &'a str,
    version: &'a str,
    arch: &'a str,
}

/// 调用 `dpkg-deb --build` 生成产物。
fn run_dpkg_deb(job: &DpkgBuild<'_>) -> Result<PathBuf> {
    let DpkgBuild {
        work,
        out_dir,
        cancel,
        name,
        version,
        arch,
    } = job;
    // `dpkg-deb` 是子进程，中途终止会留下半个产物，因此只在启动前检查取消。
    ensure_not_cancelled(cancel)?;

    fs::create_dir_all(out_dir)
        .map_err(|source| BackendError::io("创建输出目录", out_dir.display(), source))?;
    let file_name = format!("{name}_{version}_{arch}_repack.deb");
    let output = out_dir.join(sanitize_file_component(&file_name));
    let _ = fs::remove_file(&output);

    let mut command = Command::new("dpkg-deb");
    command
        .arg("--root-owner-group")
        .arg("--build")
        .arg(work)
        .arg(&output);
    let result = run_command(&mut command, Duration::from_mins(30))
        .map_err(|source| BackendError::command("构建安装包", source))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(BackendError::CommandFailed {
            operation: "构建安装包",
            code: result.status.code().unwrap_or(-1),
            detail: stderr.trim().to_string(),
        });
    }
    Ok(output)
}

/// 设置权限位（deb 对 `DEBIAN/control` 等有明确要求）。
pub(crate) fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|source| BackendError::io("设置文件权限", path.display(), source))
}

/// 供各阶段使用的取消检查。
pub(crate) fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(BackendError::Cancelled {
            operation: "构建安装包",
        });
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
            .find(|package| {
                dpkg::installed_files(&layout, &package.metadata.name, &package.metadata.arch)
                    .is_some_and(|files| (5..500).contains(&files.len()))
            })
            .expect("应有一个文件数适中的已安装包");

        let out_dir = std::env::temp_dir().join(format!("debdeck-repack-{}", std::process::id()));
        let cancel = AtomicBool::new(false);
        let outcome = build_package(BuildOptions {
            package,
            layout: &layout,
            out_dir: &out_dir,
            cancel: &cancel,
            progress: &mut |_| {},
        })
        .expect("重打包应成功");
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
