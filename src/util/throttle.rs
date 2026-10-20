//! 事件节流：把高频回调压到固定间隔，同时保证第一次一定放行。
//!
//! 下载的字节回调每 128 KiB 一次、重打包的文件回调每个文件一次，直接转成
//! UI 事件会淹掉事件通道并让界面卡顿；只保留「最近一次」的语义就够了。

use std::time::{Duration, Instant};

pub struct Throttle {
    interval: Duration,
    last: Option<Instant>,
}

impl Throttle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// 现在该不该发送？首次调用总是放行，之后距上次放行超过间隔才放行。
    pub fn touch(&mut self) -> bool {
        let due = match self.last {
            None => true,
            Some(last) => last.elapsed() >= self.interval,
        };
        if due {
            self.last = Some(Instant::now());
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_always_passes() {
        let mut throttle = Throttle::new(Duration::from_secs(60));
        assert!(throttle.touch());
        assert!(!throttle.touch());
    }

    #[test]
    fn passes_again_after_interval() {
        let mut throttle = Throttle::new(Duration::from_millis(5));
        assert!(throttle.touch());
        assert!(!throttle.touch());
        std::thread::sleep(Duration::from_millis(10));
        assert!(throttle.touch());
    }
}
