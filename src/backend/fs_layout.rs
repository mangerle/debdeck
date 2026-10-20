//! 系统文件布局的注入点。
//!
//! 后端过去把 `/var/lib/dpkg/status` 等绝对路径写成常量，导致解析逻辑只能靠
//! 真机测试（`#[ignore]` 的集成测试）。这里把这些位置收敛成一个可注入的
//! 结构：[`FsLayout::system`] 用于生产，[`FsLayout::with_root`] 指向一个
//! 镜像了相同相对结构的 fixture 目录。

use std::path::PathBuf;

/// 后端读取到的系统数据位置。
#[derive(Clone, Debug)]
pub struct FsLayout {
    /// dpkg 状态数据库（所有已安装包的 deb822 记录）。
    pub dpkg_status: PathBuf,
    /// dpkg info 目录：`<包>.list`、`<包>.md5sums`、维护脚本等。
    pub dpkg_info_dir: PathBuf,
    /// APT 下载后的软件包索引目录。
    pub apt_lists_dir: PathBuf,
    /// 传统单文件源配置。
    pub apt_sources_list: PathBuf,
    /// 源配置目录（`.list` 与 deb822 格式的 `.sources` 混放）。
    pub apt_sources_dir: PathBuf,
}

impl FsLayout {
    /// 真实 Ubuntu 系统布局。
    pub fn system() -> Self {
        Self {
            dpkg_status: PathBuf::from("/var/lib/dpkg/status"),
            dpkg_info_dir: PathBuf::from("/var/lib/dpkg/info"),
            apt_lists_dir: PathBuf::from("/var/lib/apt/lists"),
            apt_sources_list: PathBuf::from("/etc/apt/sources.list"),
            apt_sources_dir: PathBuf::from("/etc/apt/sources.list.d"),
        }
    }

    /// 以 `root` 为前缀构造布局，供 fixture 测试使用。
    ///
    /// fixture 只需按 `/var/lib/dpkg/…`、`/var/lib/apt/lists`、`/etc/apt/…`
    /// 的相对结构摆放文件即可。
    #[cfg(test)]
    pub fn with_root(root: &std::path::Path) -> Self {
        Self {
            dpkg_status: root.join("var/lib/dpkg/status"),
            dpkg_info_dir: root.join("var/lib/dpkg/info"),
            apt_lists_dir: root.join("var/lib/apt/lists"),
            apt_sources_list: root.join("etc/apt/sources.list"),
            apt_sources_dir: root.join("etc/apt/sources.list.d"),
        }
    }
}

impl Default for FsLayout {
    fn default() -> Self {
        Self::system()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn root_layout_preserves_system_relative_structure() {
        let root = Path::new("/tmp/fixture");
        let fixture = FsLayout::with_root(root);
        assert_eq!(fixture.dpkg_status, root.join("var/lib/dpkg/status"));
        assert_eq!(fixture.dpkg_info_dir, root.join("var/lib/dpkg/info"));
        assert_eq!(fixture.apt_lists_dir, root.join("var/lib/apt/lists"));
        assert_eq!(fixture.apt_sources_list, root.join("etc/apt/sources.list"));
        assert_eq!(fixture.apt_sources_dir, root.join("etc/apt/sources.list.d"));
    }
}
