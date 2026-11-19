//! D-Bus 认证代理注册与回调实现。

use std::collections::HashMap;
use std::result::Result;
use std::time::Duration;

use crate::backend::{BackendError, Result as BackendResult};
use zbus::connection::Builder;
use zbus::{Connection, OwnedVariant, Proxy, Variant, block_on};

use super::AGENT_PATH;
use super::controller::{AuthOutcome, AuthRequest, ControllerEvent};

const AUTHORITY_SERVICE: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
/// D-Bus 方法调用时限，避免 polkitd 无响应时永久挂起。
const DBUS_METHOD_TIMEOUT: Duration = Duration::from_secs(10);

/// 注册成功后持有的连接与 subject；drop 时通知 polkitd 注销。
pub(super) struct AgentRegistration {
    proxy: Proxy<'static>,
    pid: u32,
    start_time: u64,
    uid: i32,
}

impl Drop for AgentRegistration {
    fn drop(&mut self) {
        let subject = subject_value(self.pid, self.start_time, self.uid);
        let _ = block_on(
            self.proxy
                .call_method("UnregisterAuthenticationAgent", &(subject, AGENT_PATH)),
        );
    }
}

/// 注册代理，把 polkitd 的回调直接转发到安装控制器的事件总线。
pub(super) fn register_agent(
    pid: u32,
    start_time: u64,
    uid: i32,
    tx: async_channel::Sender<ControllerEvent>,
) -> BackendResult<AgentRegistration> {
    let connection = create_agent_connection(AgentIface { tx })?;
    let proxy = create_authority_proxy(connection)?;
    let subject = subject_value(pid, start_time, uid);
    let locale = std::env::var("LANG").unwrap_or_else(|_| "C".to_string());
    block_on(proxy.call_method(
        "RegisterAuthenticationAgent",
        &(subject, locale, AGENT_PATH),
    ))
    .map_err(|error| BackendError::Dbus {
        operation: "注册 polkit 认证代理",
        detail: error.to_string(),
    })?;
    tracing::info!("已注册内嵌 polkit 认证代理（安装进程 pid {pid}）");
    Ok(AgentRegistration {
        proxy,
        pid,
        start_time,
        uid,
    })
}

fn create_agent_connection(iface: AgentIface) -> BackendResult<Connection> {
    block_on(async {
        let connection = Builder::system()
            .method_timeout(DBUS_METHOD_TIMEOUT)
            .build()
            .await
            .map_err(|error| BackendError::Dbus {
                operation: "连接系统总线",
                detail: error.to_string(),
            })?;
        connection
            .object_server()
            .at(AGENT_PATH, iface)
            .await
            .map_err(|error| BackendError::Dbus {
                operation: "导出 polkit 认证代理对象",
                detail: error.to_string(),
            })?;
        Ok(connection)
    })
}

fn create_authority_proxy(connection: Connection) -> BackendResult<Proxy<'static>> {
    block_on(Proxy::new_owned(
        connection,
        AUTHORITY_SERVICE,
        AUTHORITY_PATH,
        AUTHORITY_IFACE,
    ))
    .map_err(|error| BackendError::Dbus {
        operation: "连接 PolicyKit",
        detail: error.to_string(),
    })
}

fn subject_value(
    pid: u32,
    start_time: u64,
    uid: i32,
) -> (&'static str, HashMap<&'static str, Variant<'static>>) {
    let mut details = HashMap::with_capacity(3);
    details.insert("pid", Variant::from(pid));
    details.insert("start-time", Variant::from(start_time));
    details.insert("uid", Variant::from(uid));
    ("unix-process", details)
}

/// polkit 的标准错误名；取消必须用 `Cancelled`，polkitd 才能识别为用户取消。
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error", impl_display = true)]
enum PolicyKitError {
    Cancelled(String),
    Failed(String),
}

struct AgentIface {
    tx: async_channel::Sender<ControllerEvent>,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl AgentIface {
    // 参数必须按值接收：类型由 D-Bus 反序列化生成，无法借用。
    #[allow(
        clippy::needless_pass_by_value,
        clippy::too_many_arguments,
        reason = "参数列表由 D-Bus 认证代理接口固定，zbus 要求按接口签名接收各参数"
    )]
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedVariant>)>,
    ) -> Result<(), PolicyKitError> {
        tracing::info!("收到 polkit 认证请求：{action_id}");
        let _ = (icon_name, details);
        let (done, done_rx) = async_channel::bounded(1);
        if self
            .tx
            .send(ControllerEvent::Auth(AuthRequest::Begin {
                cookie,
                message,
                identities,
                done,
            }))
            .await
            .is_err()
        {
            return Err(PolicyKitError::Failed("安装进程已退出".to_string()));
        }
        match done_rx.recv().await {
            Ok(AuthOutcome::Authorized) => Ok(()),
            Ok(AuthOutcome::Failed(reason)) => Err(PolicyKitError::Failed(reason)),
            Ok(AuthOutcome::Cancelled) | Err(_) => {
                Err(PolicyKitError::Cancelled("认证已取消".to_string()))
            }
        }
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "D-Bus 取消接口要求按签名接收拥有所有权的 cookie 字符串"
    )]
    fn cancel_authentication(&self, cookie: String) {
        let _ = cookie;
        if self
            .tx
            .try_send(ControllerEvent::Auth(AuthRequest::Cancel))
            .is_err()
        {
            tracing::warn!("polkit 取消请求未能送入安装控制线程");
        }
    }
}
