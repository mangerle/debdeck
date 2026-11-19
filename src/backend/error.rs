//! 后端统一错误契约。
//!
//! 后端位于系统 I/O 边界，错误必须保留操作名称、目标路径或外部服务等现场信息，
//! 同时向调用方暴露稳定的 `thiserror` 枚举，而不是泄漏第三方错误类型或宽泛的
//! 宽泛的字符串错误。该枚举按错误语义分类；具体操作上下文由字段携带，调用方可按
//! 变体决定回退、提示或中止流程。

use std::io;
use std::result::Result as StdResult;

use thiserror::Error;

use crate::backend::deb_file::DebFileError;
use crate::util::CommandError;

/// 后端统一结果类型。
pub(crate) type Result<T> = StdResult<T, BackendError>;

/// 后端领域错误。
#[derive(Debug, Error)]
pub(crate) enum BackendError {
    /// 调用参数不符合业务约束。
    #[error("参数无效（操作：{operation}）：{detail}")]
    InvalidInput {
        /// 失败操作。
        operation: &'static str,
        /// 具体原因。
        detail: String,
    },
    /// 必需的外部程序不存在。
    #[error("未找到外部程序 {program}（操作：{operation}）")]
    MissingProgram {
        /// 失败操作。
        operation: &'static str,
        /// 程序名或绝对路径。
        program: &'static str,
    },
    /// 子进程启动或等待失败。
    #[error("执行外部程序失败（操作：{operation}）：{source}")]
    Command {
        /// 失败操作。
        operation: &'static str,
        /// 受控子进程错误。
        #[source]
        source: CommandError,
    },
    /// 外部程序正常启动但返回失败状态。
    #[error("外部程序返回失败（操作：{operation}，退出码：{code}）：{detail}")]
    CommandFailed {
        /// 失败操作。
        operation: &'static str,
        /// 退出码；平台无法提供时为 -1。
        code: i32,
        /// 合并后的诊断输出。
        detail: String,
    },
    /// 文件系统操作失败。
    #[error("文件操作失败（操作：{operation}，路径：{path}）：{source}")]
    Io {
        /// 失败操作。
        operation: &'static str,
        /// 目标路径。
        path: String,
        /// 操作系统错误。
        #[source]
        source: io::Error,
    },
    /// 网络请求或 HTTP 客户端初始化失败。
    #[error("网络操作失败（操作：{operation}，地址：{url}）：{detail}")]
    Network {
        /// 失败操作。
        operation: &'static str,
        /// 请求地址；初始化错误时可为空。
        url: String,
        /// 已脱敏的诊断信息。
        detail: String,
    },
    /// 下载内容或重建产物的完整性校验失败。
    #[error("完整性校验失败（操作：{operation}）：{detail}")]
    Integrity {
        /// 失败操作。
        operation: &'static str,
        /// 不包含凭证的校验详情。
        detail: String,
    },
    /// 用户或上层请求取消了操作。
    #[error("{operation}已取消")]
    Cancelled {
        /// 被取消的操作。
        operation: &'static str,
    },
    /// 后台线程无法创建或失败收尾。
    #[error("后台线程操作失败（操作：{operation}）：{detail}")]
    Thread {
        /// 失败操作。
        operation: &'static str,
        /// 线程系统或运行时错误。
        detail: String,
    },
    /// D-Bus 连接或方法调用失败。
    #[error("D-Bus 操作失败（操作：{operation}）：{detail}")]
    Dbus {
        /// 失败操作。
        operation: &'static str,
        /// 总线返回的诊断信息。
        detail: String,
    },
    /// polkit/PAM 认证流程失败。
    #[error("管理员认证失败：{detail}")]
    Authentication {
        /// 不含密码与 cookie 的诊断信息。
        detail: String,
    },
    /// 本地安装包预检失败。
    #[error("安装包预检失败：{source}")]
    Package {
        /// 本地安装包错误。
        #[source]
        source: DebFileError,
    },
}

impl BackendError {
    /// 构造参数错误。
    pub(crate) fn invalid(operation: &'static str, detail: impl Into<String>) -> Self {
        Self::InvalidInput {
            operation,
            detail: detail.into(),
        }
    }

    /// 构造外部程序缺失错误。
    pub(crate) fn missing_program(operation: &'static str, program: &'static str) -> Self {
        Self::MissingProgram { operation, program }
    }

    /// 构造外部程序失败错误。
    pub(crate) fn command(operation: &'static str, source: CommandError) -> Self {
        Self::Command { operation, source }
    }

    /// 构造文件系统错误。
    #[allow(
        clippy::needless_pass_by_value,
        reason = "允许调用方直接传入 Path、Display 或临时字符串，避免在错误构造处重复分配"
    )]
    pub(crate) fn io(operation: &'static str, path: impl ToString, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_string(),
            source,
        }
    }

    /// 构造网络错误。
    pub(crate) fn network(
        operation: &'static str,
        url: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::Network {
            operation,
            url: url.into(),
            detail: detail.into(),
        }
    }
}
