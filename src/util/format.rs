//! 面向用户的数值与时间格式化。

use std::time::SystemTime;

use chrono::{DateTime, Local};

/// 把字节数格式化为二进制前缀（1024 进制）的可读字符串。
///
/// 小数位按数量级收缩：>=100 不留小数、>=10 留一位、其余留两位，
/// 末尾的 0 与孤立小数点会被去掉。
///
/// 这里的 `u64 as f64` 转换只用于显示，第 16 位之后的精度损失不可见，
/// 因此定点放行 `cast_precision_loss` 而不是引入十进制库。
#[allow(
    clippy::cast_precision_loss,
    reason = "u64 转 f64 仅用于人类可读展示，显示精度损失可以接受"
)]
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    let mut text = format!("{value:.digits$}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    format!("{text} {}", UNITS[unit])
}

/// 把摘要字节渲染成小写十六进制串（SHA256 校验与 md5sums 共用）。
pub fn hex_lower(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // 手写而不依赖 `lowerhex`，避免为一个工具函数增加依赖。
        text.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        text.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
    }
    text
}

/// dpkg 的 `Installed-Size` 字段以 KiB 为单位，展示前先换算成字节。
/// `saturating_mul` 防止畸形索引里的超大值溢出。
pub fn format_kib(kib: u64) -> String {
    format_bytes(kib.saturating_mul(1024))
}

/// 把可选的时间戳渲染成本地时间；缺失时用全角破折号占位，避免界面出现空白。
pub fn format_time(time: Option<SystemTime>) -> String {
    match time {
        Some(time) => {
            let dt: DateTime<Local> = time.into();
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        }
        None => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(10 * 1024 * 1024), "10 MB");
    }

    #[test]
    fn formats_kib() {
        assert_eq!(format_kib(1), "1 KB");
        assert_eq!(format_kib(2048), "2 MB");
    }
}
