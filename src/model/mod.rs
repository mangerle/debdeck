//! 与 IO 无关的领域数据。
//!
//! 这里只描述「软件包世界长什么样」：dpkg 状态、软件源条目、桌面条目，以及
//! 把已安装数据与软件源数据合并成界面条目（[`build_entries`]）的规则。
//! 读写系统的部分在 `backend`，线程与事件在 `runtime`，本模块全部是纯数据与纯函数。

mod catalog_builder;
mod package;
mod repository;

pub use catalog_builder::{AppEntry, build_entries};
pub use package::{
    DebFile, DesktopEntry, DpkgState, InstalledData, InstalledPackage, MenuEntryState,
    PackageMetadata, RepoPackage,
};
pub use repository::RepositoryData;
