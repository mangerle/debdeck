//! 监视外部 dpkg/apt 操作，让界面在别人改动系统后跟上。
//!
//! # 为什么是轮询而不是 inotify
//!
//! apt/dpkg 写 `/var/lib/dpkg/status` 的方式是「写临时文件 + rename」，inotify
//! 在 rename 事件上给出的路径与 watch 目标不一致，处理起来需要额外一轮目录监听；
//! 而这里只需要「变更发生了」这一个布尔信号，2 秒一轮询、成本是一次 `stat`。
//! 引入 `notify` 依赖换取的收益不显著，因此保持轮询并把它做成显式可停止的组件。
//!
//! # 沉降时间
//!
//! 一次 apt 事务会连续触碰多个文件，检测到变化后再等一小段时间才上报，
//! 避免为同一件事触发多次刷新。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::backend::dpkg::dpkg_status_mtime;
use crate::backend::fs_layout::FsLayout;
use crate::runtime::events::{BackendEvent, EventSink};

/// 轮询间隔。
const POLL: Duration = Duration::from_secs(2);
/// 检测到变化后的沉降时间。
const SETTLE: Duration = Duration::from_millis(600);

/// 一个可停止的状态监视器。句柄被丢弃（[`Drop`]）时先发停止信号再等待线程结束。
pub struct Watcher {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Watcher {
    pub fn start(layout: FsLayout, sink: EventSink) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || run(&layout, &sink, &thread_stop));
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            tracing::error!("dpkg 状态监视线程发生 panic，已隔离");
        }
    }
}

fn run(layout: &FsLayout, sink: &EventSink, stop: &AtomicBool) {
    let mut last_mtime = dpkg_status_mtime(layout);
    while !stop.load(Ordering::Relaxed) {
        if sleep_responsive(stop, POLL) {
            break;
        }
        let current = dpkg_status_mtime(layout);
        // 读不到（apt 正持有、或文件被临时移走）时保留上次值，
        // 下一轮若mtime 变了仍会上报，不会丢事件。
        if current.is_none() || current == last_mtime {
            continue;
        }
        last_mtime = current;
        if sleep_responsive(stop, SETTLE) {
            break;
        }
        sink.send(BackendEvent::DpkgChanged);
    }
}

/// 分段睡眠，使停止请求在 [`POLL`] 内被响应，而不是等完整一个间隔。
/// 返回 `true` 表示收到停止请求。
fn sleep_responsive(stop: &AtomicBool, duration: Duration) -> bool {
    const STEP: Duration = Duration::from_millis(200);
    let mut remaining = duration;
    while !stop.load(Ordering::Relaxed) {
        if remaining <= STEP {
            if !remaining.is_zero() {
                thread::sleep(remaining);
            }
            return stop.load(Ordering::Relaxed);
        }
        thread::sleep(STEP);
        remaining -= STEP;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn stop_flag_ends_sleep() {
        let stop = AtomicBool::new(true);
        assert!(sleep_responsive(&stop, Duration::from_millis(50)));
    }

    #[test]
    fn waits_full_duration_when_not_stopped() {
        let stop = AtomicBool::new(false);
        let start = std::time::Instant::now();
        assert!(!sleep_responsive(&stop, Duration::from_millis(40)));
        assert!(start.elapsed() >= Duration::from_millis(35));
    }

    /// 轮询判定：只有「读得到」且「与上次不同」才算变化。
    fn is_change(last: Option<SystemTime>, current: Option<SystemTime>) -> bool {
        current.is_some() && current != last
    }

    #[test]
    fn unreadable_mtime_is_not_an_event() {
        let now = SystemTime::now();
        assert!(!is_change(Some(now), None));
        assert!(!is_change(Some(now), Some(now)));
        assert!(is_change(Some(now), Some(now + Duration::from_secs(1))));
    }
}
