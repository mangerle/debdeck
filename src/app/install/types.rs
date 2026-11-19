//! 独立安装窗口的状态枚举、阶段进度模型与输出行解析。

use crate::model::DebFile;
use crate::runtime::install::InstalledStatus;

/// 安装窗口的流程。
#[derive(Clone)]
pub(crate) enum Phase {
    /// 展示包信息，等待用户点「安装」。
    Confirm,
    /// 已启动 pkexec，等待 PAM 提示（或直接授权成功）。
    Authorizing,
    /// PAM 要求输入，密码框就在窗口里。
    NeedPassword {
        message: String,
        prompt: String,
        echo: bool,
        error: Option<String>,
    },
    /// 授权成功，apt-get 正在执行。
    Installing,
    Done(Result<String, String>),
}

/// 安装过程中的实时阶段进度。
#[derive(Clone, Debug)]
pub(crate) struct InstallStage {
    /// 给用户看的阶段描述。
    pub(crate) label: &'static str,
    /// 估算进度（`0.0..=1.0`）。
    pub(crate) progress: f32,
}

impl Default for InstallStage {
    fn default() -> Self {
        Self {
            label: "授权成功，正在准备安装…",
            progress: 0.15,
        }
    }
}

/// 安装包预检状态。
#[derive(Clone)]
pub(crate) enum InstallFile {
    /// 正在后台解析 `.deb` 元数据。
    Inspecting,
    /// 预检通过。
    Ready {
        file: Box<DebFile>,
        installed: InstalledStatus,
    },
    /// 预检失败。
    Failed(String),
}

/// 详情抽屉的当前展开目标（互斥状态，避免多个布尔标志）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DetailDrawer {
    None,
    Logs,
    AuthCommand,
}

/// 渲染时需要一次性应用的密码框操作。
pub(crate) struct PasswordUi {
    pub(crate) clear: bool,
    pub(crate) focus: bool,
    pub(crate) mask: Option<bool>,
}

/// 从 `apt-get` / `dpkg` 输出行中识别当前安装阶段与估算进度。
pub(super) fn infer_install_stage(line: &str) -> Option<InstallStage> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("reading package lists") || line.contains("正在读取软件包列表") {
        return Some(InstallStage {
            label: "正在读取软件包列表…",
            progress: 0.25,
        });
    }
    if lower.contains("building dependency tree") || line.contains("正在分析软件包的依赖关系树")
    {
        return Some(InstallStage {
            label: "正在分析依赖关系…",
            progress: 0.35,
        });
    }
    if lower.contains("get:") || lower.contains("fetching") || line.contains("获取:") {
        return Some(InstallStage {
            label: "正在下载依赖软件包…",
            progress: 0.50,
        });
    }
    if lower.contains("preparing to unpack") || line.contains("正准备解包") {
        return Some(InstallStage {
            label: "准备解包软件文件…",
            progress: 0.62,
        });
    }
    if lower.contains("unpacking ") || line.contains("正在解包") {
        return Some(InstallStage {
            label: "正在解包并写入文件…",
            progress: 0.72,
        });
    }
    if lower.contains("setting up ") || line.contains("正在设置") {
        return Some(InstallStage {
            label: "正在配置软件包…",
            progress: 0.86,
        });
    }
    if lower.contains("processing triggers") || line.contains("正在处理用于") {
        return Some(InstallStage {
            label: "正在更新系统缓存与菜单…",
            progress: 0.95,
        });
    }
    None
}
