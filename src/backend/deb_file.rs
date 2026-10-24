//! 本地 `.deb` 文件的预检：路径规范化与控制字段解析。
//!
//! 提权安装只接受经过本模块检查的路径：必须是存在的普通文件、以 `.deb` 结尾，
//! 且能被 `dpkg-deb` 解析出控制信息。确认框因此可以展示包名、版本与大小，
//! 而不是让用户对一个陌生文件名点「安装」。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::deb822::parse_stanzas;
use crate::model::{DebFile, PackageMetadata};
use crate::util::command_exists;

/// 校验路径并读取控制信息。
pub fn inspect(path: &Path) -> Result<DebFile> {
    let path = canonicalize(path)?;
    if !command_exists("dpkg-deb") {
        bail!("未找到 dpkg-deb，无法读取安装包信息。");
    }
    let output = Command::new("dpkg-deb")
        .arg("--field")
        .arg(&path)
        .output()
        .context("无法运行 dpkg-deb")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if detail.is_empty() {
            bail!("无法解析安装包：{}", path.display());
        }
        bail!("无法解析安装包：{}（{detail}）", path.display());
    }

    let control = String::from_utf8_lossy(&output.stdout);
    let stanza = parse_stanzas(&control)
        .into_iter()
        .next()
        .context("安装包缺少控制信息")?;
    let metadata = PackageMetadata::from_stanza(&stanza);
    if metadata.name.is_empty() {
        bail!("安装包缺少 Package 字段：{}", path.display());
    }
    let file_size = fs::metadata(&path)
        .with_context(|| format!("无法读取 {}", path.display()))?
        .len();
    Ok(DebFile {
        path,
        metadata,
        file_size,
    })
}

/// 路径必须是真实存在的 `.deb` 普通文件；返回解析过符号链接的绝对路径。
pub fn canonicalize(path: &Path) -> Result<PathBuf> {
    let resolved = path
        .canonicalize()
        .with_context(|| format!("找不到安装包：{}", path.display()))?;
    if !resolved.is_file() {
        bail!("不是可安装的文件：{}", resolved.display());
    }
    let is_deb = resolved
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("deb"));
    if !is_deb {
        bail!("不是 .deb 安装包：{}", resolved.display());
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

        assert!(format!("{error:#}").contains("不是 .deb"), "{error:#}");
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
        let status = Command::new("dpkg-deb")
            .args(["--build", "--root-owner-group"])
            .arg(&pkg)
            .arg(&deb)
            .status()
            .unwrap();
        assert!(status.success(), "dpkg-deb 构建测试包失败");

        let info = inspect(&deb).unwrap();

        assert_eq!(info.metadata.name, "debdeck-test");
        assert_eq!(info.metadata.version, "1.2.3-1");
        assert_eq!(info.metadata.arch, "all");
        assert_eq!(info.metadata.summary, "test package");
        assert!(info.file_size > 0);
        assert!(info.path.is_absolute());
        let _ = fs::remove_dir_all(&root);
    }
}
