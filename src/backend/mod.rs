//! 后端层：唯一允许触碰真实系统的地方。
//!
//! # 职责与边界
//!
//! 本层负责读取 dpkg/apt 数据、解析 freedesktop 桌面条目、执行有副作用的
//! 系统操作（pkexec、HTTP 下载、重打包）。它**不允许**依赖 GPUI、`app` 或
//! `ui`；需要通知上层时，由 `runtime` 负责把结果转成事件。
//!
//! # 可测试性
//!
//! 所有文件系统位置通过 [`FsLayout`] 注入：生产路径指向真实系统，测试指向
//! fixture 根目录。因此新增读取逻辑时不要再写绝对路径常量。
//!
//! # 子模块
//!
//! * `fs_layout`  —— 路径注入点
//! * `deb822`     —— deb822 段落格式的唯一解析器
//! * `dpkg`       —— /var/lib/dpkg 的 status 与 info 目录
//! * `apt`        —— /etc/apt 源配置与 /var/lib/apt/lists 索引
//! * `desktop`    —— .desktop 解析、图标查找、启动
//! * `shell`      —— 桌面集成副作用（通知、打开文件管理器）
//! * `privileged` —— 经 pkexec 提权的包操作
//! * `deb_file`   —— 本地 `.deb` 预检与控制信息解析
//! * `polkit_agent` —— 内嵌 polkit 认证代理（密码框留在安装窗口）
//! * `download`   —— 在线获取安装包
//! * `repack`     —— 从已安装文件离线重建 deb

pub mod apt;
pub mod deb_file;
pub mod desktop;
pub mod download;
pub mod dpkg;
pub mod fs_layout;
pub mod polkit_agent;
pub mod privileged;
pub mod repack;
pub mod shell;

pub use fs_layout::FsLayout;
