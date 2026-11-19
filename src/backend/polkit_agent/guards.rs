//! 提权控制器的后台守卫：UI 回执转发与总超时控制。
//!
//! 两个守卫都遵循 RAII 约定，在控制器退出时自动唤醒并回收后台线程。

use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::AuthResponse;
use super::controller::ControllerEvent;

/// 将 UI 密码回执通道转发到控制器事件总线；通道关闭时发送 `Stop` 立即唤醒控制器。
pub(super) struct ResponseForwarder {
    responses: async_channel::Receiver<AuthResponse>,
    handle: Option<JoinHandle<()>>,
}

impl ResponseForwarder {
    pub(super) fn start(
        responses: async_channel::Receiver<AuthResponse>,
        ctrl_tx: async_channel::Sender<ControllerEvent>,
    ) -> Self {
        let rx = responses.clone();
        let handle = thread::spawn(move || {
            while let Ok(response) = rx.recv_blocking() {
                if ctrl_tx
                    .send_blocking(ControllerEvent::Response(response))
                    .is_err()
                {
                    return;
                }
            }
            let _ = ctrl_tx.send_blocking(ControllerEvent::Stop);
        });
        Self {
            responses,
            handle: Some(handle),
        }
    }
}

impl Drop for ResponseForwarder {
    fn drop(&mut self) {
        self.responses.close();
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            tracing::warn!("安装回执转发线程发生 panic，已隔离");
        }
    }
}

/// 安装总超时守卫：基于 `Condvar` 阻塞等待，退出时瞬间唤醒回收。
pub(super) struct TimeoutGuard {
    state: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}

impl TimeoutGuard {
    pub(super) fn start(
        timeout: Duration,
        ctrl_tx: async_channel::Sender<ControllerEvent>,
    ) -> Self {
        let state = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_state = Arc::clone(&state);
        let handle = thread::spawn(move || {
            let (lock, condvar) = &*thread_state;
            let Ok(guard) = lock.lock() else { return };
            let result = condvar.wait_timeout_while(guard, timeout, |cancelled| !*cancelled);
            if let Ok((cancelled, _)) = result
                && !*cancelled
            {
                let _ = ctrl_tx.send_blocking(ControllerEvent::Timeout);
            }
        });
        Self {
            state,
            handle: Some(handle),
        }
    }
}

impl Drop for TimeoutGuard {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.state;
        if let Ok(mut cancelled) = lock.lock() {
            *cancelled = true;
            condvar.notify_all();
        }
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            tracing::warn!("安装超时守卫线程发生 panic，已隔离");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_response_sender_wakes_forwarder_with_stop() {
        let (response_tx, response_rx) = async_channel::bounded(1);
        let (ctrl_tx, ctrl_rx) = async_channel::bounded(1);
        let forwarder = ResponseForwarder::start(response_rx, ctrl_tx);

        drop(response_tx);

        assert!(
            matches!(ctrl_rx.recv_blocking(), Ok(ControllerEvent::Stop)),
            "关闭回执通道必须唤醒控制器停止"
        );
        drop(forwarder);
    }
}
