//! `DEBIAN/control` 与包内控制文件的生成。
//!
//! control 的字段直接抄 dpkg status 里的原始段落，但要做两件事：
//!
//! * 丢弃 dpkg 私有字段（`Status`、`Config-Version` 等），只保留控制字段；
//! * 用实际复制的字节数重算 `Installed-Size` —— 沿用 status 里的旧值会让安装后
//!   占用与包内声明不一致。

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::backend::{BackendError, Result};

use crate::backend::dpkg::info_file;
use crate::backend::fs_layout::FsLayout;
use crate::deb822::{Stanza, write_field};
use crate::model::{InstalledPackage, PackageMetadata};

use super::set_mode;

/// control 允许出现的字段白名单。
///
/// dpkg status 里还混有 `Status`、`Config-Version` 等本地状态字段，直接抄过去会
/// 让 `dpkg-deb --build` 或安装阶段出错，因此只保留规范定义的控制字段。
const CONTROL_FIELDS: &[&str] = &[
    "Package",
    "Source",
    "Version",
    "Architecture",
    "Maintainer",
    "Original-Maintainer",
    "Installed-Size",
    "Depends",
    "Pre-Depends",
    "Recommends",
    "Suggests",
    "Breaks",
    "Conflicts",
    "Replaces",
    "Provides",
    "Enhances",
    "Section",
    "Priority",
    "Essential",
    "Build-Essential",
    "Multi-Arch",
    "Tag",
    "Description",
    "Homepage",
    "Bugs",
    "Protected",
    "Built-Using",
    "Static-Built-Using",
    "Installer-Menu-Item",
    "Kernel-Version",
    "Package-Type",
    "Subarchitecture",
    "Built-For-Profiles",
];

/// dpkg 要求必须存在的字段，缺失时用元数据补写。
const REQUIRED_FIELDS: [&str; 6] = [
    "Package",
    "Version",
    "Architecture",
    "Maintainer",
    "Installed-Size",
    "Description",
];

/// 维护脚本与附加控制文件；脚本需要可执行位。
const EXTRA_CONTROL_FILES: [(&str, u32); 8] = [
    ("preinst", 0o755),
    ("postinst", 0o755),
    ("prerm", 0o755),
    ("postrm", 0o755),
    ("conffiles", 0o644),
    ("triggers", 0o644),
    ("shlibs", 0o644),
    ("symbols", 0o644),
];

/// 控制文件生成所需的完整输入。
pub struct ControlWrite<'a> {
    /// 产物中的 `DEBIAN` 目录。
    pub debian_dir: &'a Path,
    /// 已安装软件包记录。
    pub package: &'a InstalledPackage,
    /// 按需读取的原始控制段落。
    pub control: &'a Stanza,
    /// 系统路径布局。
    pub layout: &'a FsLayout,
    /// 已复制文件的总字节数。
    pub data_bytes: u64,
    /// 阶段进度回调。
    pub progress: &'a mut dyn FnMut(&str),
}

/// 写出 `DEBIAN/control`、维护脚本等控制文件。
pub fn write_control(input: ControlWrite<'_>) -> Result<()> {
    let ControlWrite {
        debian_dir,
        package,
        control,
        layout,
        data_bytes,
        progress,
    } = input;
    progress("正在收集软件包控制信息…");
    fs::create_dir_all(debian_dir)
        .map_err(|source| BackendError::io("创建控制文件目录", debian_dir.display(), source))?;
    let installed_size_kb = data_bytes.div_ceil(1024);
    let control = build_control(&package.metadata, control, installed_size_kb);
    let control_path = debian_dir.join("control");
    fs::write(&control_path, control)
        .map_err(|source| BackendError::io("写入 control 文件", control_path.display(), source))?;
    set_mode(&control_path, 0o644)?;
    for (suffix, mode) in EXTRA_CONTROL_FILES {
        copy_extra_control_file(&ExtraControlFile {
            debian_dir,
            package,
            layout,
            suffix,
            mode,
        })?;
    }
    set_mode(debian_dir, 0o755)
}

#[derive(Clone, Copy)]
struct ExtraControlFile<'a> {
    debian_dir: &'a Path,
    package: &'a InstalledPackage,
    layout: &'a FsLayout,
    suffix: &'a str,
    mode: u32,
}

fn copy_extra_control_file(input: &ExtraControlFile<'_>) -> Result<()> {
    let ExtraControlFile {
        debian_dir,
        package,
        layout,
        suffix,
        mode,
    } = input;
    let Some(source) = info_file(
        layout,
        &package.metadata.name,
        &package.metadata.arch,
        suffix,
    ) else {
        return Ok(());
    };
    let destination = debian_dir.join(suffix);
    fs::copy(&source, &destination)
        .map_err(|source_error| BackendError::io("复制控制文件", source.display(), source_error))?;
    set_mode(&destination, *mode)
}

/// 生成 control 文本。
fn build_control(metadata: &PackageMetadata, control: &Stanza, installed_size_kb: u64) -> String {
    let mut output = String::with_capacity(control.fields.len().saturating_mul(32));
    let mut written: HashSet<&str> = HashSet::with_capacity(control.fields.len());

    for (key, value) in &control.fields {
        if !CONTROL_FIELDS.contains(&key.as_str()) {
            continue;
        }
        if key == "Installed-Size" {
            // 用实测值替换声明值。
            write_field(&mut output, key, &installed_size_kb.to_string());
        } else {
            write_field(&mut output, key, value);
        }
        written.insert(key.as_str());
    }

    for key in REQUIRED_FIELDS {
        if !written.contains(key) {
            write_field(
                &mut output,
                key,
                &fallback_value(key, metadata, installed_size_kb),
            );
        }
    }

    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// 必填字段缺失时的兜底取值。
fn fallback_value(key: &str, metadata: &PackageMetadata, installed_size_kb: u64) -> String {
    match key {
        "Package" => metadata.name.clone(),
        "Version" => metadata.version.clone(),
        "Architecture" => metadata.arch.clone(),
        "Maintainer" => {
            if metadata.maintainer.is_empty() {
                "Unknown Maintainer <unknown@localhost>".to_string()
            } else {
                metadata.maintainer.clone()
            }
        }
        "Installed-Size" => installed_size_kb.to_string(),
        "Description" => {
            if !metadata.description.is_empty() {
                metadata.description.clone()
            } else if !metadata.summary.is_empty() {
                metadata.summary.clone()
            } else {
                metadata.name.clone()
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deb822::parse_stanzas;

    fn metadata_and_control(stanza_text: &str) -> (PackageMetadata, Stanza) {
        let stanzas = parse_stanzas(stanza_text);
        let control = stanzas.into_iter().next().unwrap_or_default();
        let metadata = PackageMetadata::from_stanza(&control);
        (metadata, control)
    }

    #[test]
    fn writes_control_with_recomputed_size() {
        let (metadata, stanza) = metadata_and_control(
            "Package: demo\nStatus: install ok installed\nVersion: 1.0\nArchitecture: amd64\nMaintainer: Demo <demo@example.com>\nInstalled-Size: 999\nDescription: demo\n long\n",
        );
        let control = build_control(&metadata, &stanza, 12);
        assert!(!control.contains("Status:"), "本地状态字段不应进入 control");
        assert!(control.contains("Installed-Size: 12\n"));
        assert!(!control.contains("Installed-Size: 999"));
        assert!(control.contains("Description: demo\n long\n"));
    }

    #[test]
    fn whitelist_ignores_internal_fields() {
        let (metadata, stanza) = metadata_and_control(
            "Package: demo\nStatus: install ok installed\nConfig-Version: 2\nOriginal-Maintainer: Debian\n",
        );
        let control = build_control(&metadata, &stanza, 1);
        assert!(control.contains("Original-Maintainer: Debian"));
        assert!(!control.contains("Status:"));
        assert!(!control.contains("Config-Version:"));
    }

    #[test]
    fn adds_missing_required_fields() {
        let (metadata, stanza) =
            metadata_and_control("Package: demo\nVersion: 1.0\nDescription: only a summary\n");
        let control = build_control(&metadata, &stanza, 7);
        for field in [
            "Package:",
            "Version:",
            "Architecture:",
            "Maintainer:",
            "Installed-Size: 7",
            "Description:",
        ] {
            assert!(control.contains(field), "缺少必填字段 {field}：\n{control}");
        }
        // 段落里没有 Maintainer 时用占位值，dpkg 才肯接受这个包。
        assert!(control.contains("Unknown Maintainer"));
    }
}
