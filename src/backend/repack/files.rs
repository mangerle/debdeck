//! 把 dpkg 记录的文件清单复制到工作目录。
//!
//! 只复制、跳过，不做任何「修复」：路径必须是绝对路径且不含 `..`、绝对化
//! 符号链接按原样保留、特殊文件（设备、socket、fifo）无法放进 deb 所以跳过。
//! 每一项跳过都计数，最终随结果上报给用户，避免「打包成功但少了一半文件」
//! 这种静默降级。

use std::fs::{self, Metadata};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::Result;

use super::{PROGRESS_EVERY, ensure_not_cancelled};

/// 复制阶段的产出。
#[derive(Default)]
pub struct CopySummary {
    /// `(包内相对路径, 工作目录中的绝对路径)`，用于生成 md5sums。
    pub copied_files: Vec<(PathBuf, PathBuf)>,
    pub skipped: usize,
    /// 复制的字节总数，用来重算 `Installed-Size`。
    pub data_bytes: u64,
}

/// 复制清单里的所有文件到 `work` 下对应的相对路径。
pub fn copy_installed_files(
    files: &[String],
    work: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<CopySummary> {
    let mut summary = CopySummary::default();
    progress("正在复制已安装文件…");

    for (index, file) in files.iter().enumerate() {
        ensure_not_cancelled(cancel)?;
        if index > 0 && index % PROGRESS_EVERY == 0 {
            progress(&format!("正在复制已安装文件…已处理 {index} 个"));
        }

        let source = Path::new(file);
        let Some(relative) = safe_relative_path(source) else {
            summary.skipped += 1;
            continue;
        };
        let Ok(metadata) = fs::symlink_metadata(source) else {
            // 清单里有、磁盘上没有：卸载或手工删除过的残留。
            summary.skipped += 1;
            continue;
        };
        let destination = work.join(&relative);
        if !create_parent_dir(&destination) {
            summary.skipped += 1;
            continue;
        }

        if metadata.is_dir() {
            copy_directory(&destination, &metadata);
        } else if metadata.file_type().is_symlink() {
            if !copy_symlink(source, &destination) {
                summary.skipped += 1;
            }
        } else if metadata.is_file() {
            match copy_regular_file(source, &destination, &metadata) {
                Ok(bytes) => {
                    summary.data_bytes += bytes;
                    summary.copied_files.push((relative, destination));
                }
                Err(error) => {
                    tracing::warn!("复制 {} 失败：{error}", source.display());
                    summary.skipped += 1;
                }
            }
        } else {
            // 设备文件、socket、fifo：不属于普通 deb 包能承载的内容。
            summary.skipped += 1;
        }
    }

    Ok(summary)
}

/// 去掉开头的 `/` 并拒绝任何越界组件，得到包内相对路径。
fn safe_relative_path(source: &Path) -> Option<PathBuf> {
    if !source.is_absolute() {
        return None;
    }
    let relative = source.strip_prefix("/").unwrap_or(source);
    if relative.as_os_str().is_empty() {
        return None;
    }
    // 只允许普通组件：`.`、`..`、根、前缀路径都可能把内容写到工作目录之外。
    relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then(|| relative.to_path_buf())
}

/// 创建目标文件的父目录；失败时记录日志并返回 `false`。
fn create_parent_dir(destination: &Path) -> bool {
    let Some(parent) = destination.parent() else {
        return false;
    };
    match fs::create_dir_all(parent) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!("创建目录 {} 失败：{error}", parent.display());
            false
        }
    }
}

/// 目录只需要存在与权限正确，内容会由各自的清单条目填充。
fn copy_directory(destination: &Path, metadata: &Metadata) {
    let _ = fs::create_dir_all(destination);
    let _ = fs::set_permissions(destination, metadata.permissions());
}

fn copy_symlink(source: &Path, destination: &Path) -> bool {
    use std::os::unix::fs::symlink;

    let Ok(target) = fs::read_link(source) else {
        return false;
    };
    // 已存在同名条目（例如清单里父子路径重复）时先删掉，否则 symlink 会失败。
    let _ = fs::remove_file(destination);
    match symlink(&target, destination) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!("创建符号链接 {} 失败：{error}", destination.display());
            false
        }
    }
}

fn copy_regular_file(source: &Path, destination: &Path, metadata: &Metadata) -> Result<u64> {
    let bytes = fs::copy(source, destination)?;
    // 权限跟随源文件：可执行位、setuid 位都会影响包能否正常工作。
    let _ = fs::set_permissions(destination, metadata.permissions());
    Ok(bytes)
}
