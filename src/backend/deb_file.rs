//! 本地 `.deb` 文件的预检：路径规范化与控制字段解析。
//!
//! 提权安装只接受经过本模块检查的路径：必须是存在的普通文件、以 `.deb` 结尾，
//! 且能被 `dpkg-deb` 解析出控制信息。确认框因此可以展示包名、版本与大小，
//! 而不是让用户对一个陌生文件名点「安装」。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use thiserror::Error;

use crate::deb822::parse_stanzas;
use crate::model::{DebFile, PackageMetadata};
use crate::util::{CommandError, command_exists, run_command};

/// 本地安装包预检错误。
#[derive(Debug, Error)]
pub enum DebFileError {
    /// 路径无法规范化。
    #[error("找不到安装包 {path}：{source}")]
    Canonicalize {
        /// 用户传入路径。
        path: String,
        /// 文件系统错误。
        #[source]
        source: io::Error,
    },
    /// 解析后的路径不是普通文件。
    #[error("不是可安装的文件：{path}")]
    NotFile {
        /// 规范化后的路径。
        path: String,
    },
    /// 文件扩展名不是 `.deb`。
    #[error("不是 .deb 安装包：{path}")]
    NotDeb {
        /// 规范化后的路径。
        path: String,
    },
    /// 系统缺少 `dpkg-deb`。
    #[error("未找到 dpkg-deb，无法读取安装包信息")]
    MissingDpkgDeb,
    /// `dpkg-deb` 无法启动、超时或回收失败。
    #[error("运行 dpkg-deb 失败：{source}")]
    Command {
        /// 受控子进程错误。
        #[source]
        source: CommandError,
    },
    /// `dpkg-deb` 返回失败状态。
    #[error("dpkg-deb 无法解析安装包 {path}：{detail}")]
    InspectFailed {
        /// 安装包路径。
        path: String,
        /// 标准错误摘要。
        detail: String,
    },
    /// 安装包缺少控制段落。
    #[error("安装包 {path} 缺少控制信息")]
    MissingControl {
        /// 安装包路径。
        path: String,
    },
    /// 安装包缺少 `Package` 字段。
    #[error("安装包 {path} 缺少 Package 字段")]
    MissingPackage {
        /// 安装包路径。
        path: String,
    },
    /// 读取文件元数据失败。
    #[error("无法读取安装包 {path} 的元数据：{source}")]
    Metadata {
        /// 安装包路径。
        path: String,
        /// 文件系统错误。
        #[source]
        source: io::Error,
    },
}

/// 校验路径并读取控制信息。
///
/// # Errors
///
/// 路径不存在、不是普通 `.deb` 文件、缺少 `dpkg-deb`、控制信息无法解析或
/// 文件元数据读取失败时返回对应错误。
pub fn inspect(path: &Path) -> Result<DebFile, DebFileError> {
    let path = canonicalize(path)?;
    if !command_exists("dpkg-deb") {
        return Err(DebFileError::MissingDpkgDeb);
    }
    let mut command = Command::new("dpkg-deb");
    command.arg("--field").arg(&path);
    let output = run_command(&mut command, Duration::from_secs(30))
        .map_err(|source| DebFileError::Command { source })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(DebFileError::InspectFailed {
            path: path.display().to_string(),
            detail: if detail.is_empty() {
                "未提供诊断信息".to_string()
            } else {
                detail.to_string()
            },
        });
    }

    let control = String::from_utf8_lossy(&output.stdout);
    let stanza =
        parse_stanzas(&control)
            .into_iter()
            .next()
            .ok_or_else(|| DebFileError::MissingControl {
                path: path.display().to_string(),
            })?;
    let metadata = PackageMetadata::from_stanza(&stanza);
    if metadata.name.is_empty() {
        return Err(DebFileError::MissingPackage {
            path: path.display().to_string(),
        });
    }
    let file_size = fs::metadata(&path)
        .map_err(|source| DebFileError::Metadata {
            path: path.display().to_string(),
            source,
        })?
        .len();
    Ok(DebFile {
        path,
        metadata,
        file_size,
    })
}

/// 路径必须是真实存在的 `.deb` 普通文件；返回解析过符号链接的绝对路径。
///
/// # Errors
///
/// 路径无法规范化、不是普通文件或扩展名不是 `.deb` 时返回错误。
pub fn canonicalize(path: &Path) -> Result<PathBuf, DebFileError> {
    let resolved = path
        .canonicalize()
        .map_err(|source| DebFileError::Canonicalize {
            path: path.display().to_string(),
            source,
        })?;
    if !resolved.is_file() {
        return Err(DebFileError::NotFile {
            path: resolved.display().to_string(),
        });
    }
    let is_deb = resolved
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("deb"));
    if !is_deb {
        return Err(DebFileError::NotDeb {
            path: resolved.display().to_string(),
        });
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("debdeck-deb-file-{label}-{}", std::process::id()))
    }

    #[test]
    fn rejects_files_without_deb_suffix() {
        let root = temp_root("suffix");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("notes.txt");
        fs::write(&path, "not a package").unwrap();

        let error = inspect(&path).unwrap_err();

        assert!(error.to_string().contains("不是 .deb"), "{error}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn inspects_a_built_package() {
        if !command_exists("dpkg-deb") {
            return;
        }
        let root = temp_root("build");
        let _ = fs::remove_dir_all(&root);
        let pkg = root.join("pkg");
        fs::create_dir_all(pkg.join("DEBIAN")).unwrap();
        fs::create_dir_all(pkg.join("usr/share/doc/demo")).unwrap();
        fs::write(
            pkg.join("DEBIAN/control"),
            "Package: debdeck-test\nVersion: 1.2.3-1\nArchitecture: all\nMaintainer: Test <test@example.com>\nDescription: test package\n long description\n",
        )
        .unwrap();
        let deb = root.join("debdeck-test_1.2.3-1_all.deb");
        let mut command = Command::new("dpkg-deb");
        command
            .args(["--build", "--root-owner-group"])
            .arg(&pkg)
            .arg(&deb);
        let status = run_command(&mut command, Duration::from_secs(30))
            .unwrap()
            .status;
        assert!(status.success(), "dpkg-deb 构建测试包失败");

        let info = inspect(&deb).unwrap();

        assert_eq!(info.metadata.name, "debdeck-test");
        assert_eq!(info.metadata.version, "1.2.3-1");
        assert_eq!(info.metadata.arch.as_ref(), "all");
        assert_eq!(info.metadata.summary, "test package");
        assert!(info.file_size > 0);
        assert!(info.path.is_absolute());
        let _ = fs::remove_dir_all(&root);
    }
}
