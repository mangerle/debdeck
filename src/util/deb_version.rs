//! Debian 版本号比较（Debian Policy §5.6.12 的完整实现）。
//!
//! 版本号形如 `[epoch:]upstream[-revision]`，比较规则是逐段切分：
//!
//! 1. epoch 按整数比较，缺省为 0；
//! 2. upstream 与 revision 各自按「非数字段 -> 数字段」交替切块比较；
//! 3. 非数字块逐字符比 ASCII，但 `~` 排在一切之前（含空），空串排在任何
//!    非 `~` 字符之前 —— 这正是 `1.0~rc1 < 1.0` 的原因；
//! 4. 数字块先去掉前导 0，再比长度、后比字典序，等价于按数值比较。
//!
//! 这是本项目里最容易被写错的算法，所以单独成模块并配对照表测试；
//! 上游工具链（`dpkg --compare-versions`）虽然权威，但会引入子进程开销，
//! 软件源索引有数千个包需要排序，不适合逐次调用。

use std::cmp::Ordering;

/// 分离 epoch。epoch 必须是纯数字，否则整串视为无 epoch（与 dpkg 行为一致）。
fn split_epoch(version: &str) -> (u64, &str) {
    if let Some((epoch, rest)) = version.split_once(':')
        && !epoch.is_empty()
        && epoch.bytes().all(|b| b.is_ascii_digit())
    {
        return (epoch.parse::<u64>().unwrap_or(0), rest);
    }
    (0, version)
}

/// 分离 upstream 与 revision：以**最后一个**连字符为界。
fn split_revision(version: &str) -> (&str, &str) {
    match version.rsplit_once('-') {
        Some((upstream, revision)) => (upstream, revision),
        None => (version, ""),
    }
}

/// 单字符比较权重：空（0）最小、`~` 为 -1 以便排在空之前，其余按 ASCII。
fn char_order(byte: u8) -> i32 {
    match byte {
        0 => 0,
        b'~' => -1,
        _ => i32::from(byte),
    }
}

/// 比较 version 或 revision 的一段（不含 epoch）。
fn version_part_cmp(left: &str, right: &str) -> Ordering {
    let mut left = left.as_bytes();
    let mut right = right.as_bytes();
    loop {
        if left.is_empty() && right.is_empty() {
            return Ordering::Equal;
        }
        if let Some(order) = compare_non_numeric_blocks(&mut left, &mut right) {
            return order;
        }
        while left.first() == Some(&b'0') {
            left = &left[1..];
        }
        while right.first() == Some(&b'0') {
            right = &right[1..];
        }
        if let Some(order) = compare_numeric_blocks(&mut left, &mut right) {
            return order;
        }
    }
}

fn compare_non_numeric_blocks(left: &mut &[u8], right: &mut &[u8]) -> Option<Ordering> {
    while left.first().is_some_and(|b| !b.is_ascii_digit())
        || right.first().is_some_and(|b| !b.is_ascii_digit())
    {
        let l = next_non_digit(left.first().copied());
        let r = next_non_digit(right.first().copied());
        let order = char_order(l).cmp(&char_order(r));
        if order != Ordering::Equal {
            return Some(order);
        }
        if l != 0 {
            *left = &left[1..];
        }
        if r != 0 {
            *right = &right[1..];
        }
    }
    None
}

fn next_non_digit(byte: Option<u8>) -> u8 {
    byte.filter(|value| !value.is_ascii_digit()).unwrap_or(0)
}

fn compare_numeric_blocks(left: &mut &[u8], right: &mut &[u8]) -> Option<Ordering> {
    let left_len = left.iter().take_while(|b| b.is_ascii_digit()).count();
    let right_len = right.iter().take_while(|b| b.is_ascii_digit()).count();
    match left_len.cmp(&right_len) {
        Ordering::Equal => {
            let order = left[..left_len].cmp(&right[..right_len]);
            if order != Ordering::Equal {
                return Some(order);
            }
            *left = &left[left_len..];
            *right = &right[right_len..];
            None
        }
        other => Some(other),
    }
}

/// 比较两个 Debian 版本号，返回 `left` 相对 `right` 的顺序。
pub fn debian_version_cmp(left: &str, right: &str) -> Ordering {
    let (left_epoch, left_rest) = split_epoch(left);
    let (right_epoch, right_rest) = split_epoch(right);
    match left_epoch.cmp(&right_epoch) {
        Ordering::Equal => {}
        other => return other,
    }

    let (left_upstream, left_revision) = split_revision(left_rest);
    let (right_upstream, right_revision) = split_revision(right_rest);
    match version_part_cmp(left_upstream, right_upstream) {
        Ordering::Equal => version_part_cmp(left_revision, right_revision),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_debian_versions() {
        let cases = [
            ("1.0", "1.0", Ordering::Equal),
            ("1.0", "1.0-1", Ordering::Less),
            ("1.0~rc1", "1.0", Ordering::Less),
            ("1.0-1ubuntu1", "1.0-1ubuntu2", Ordering::Less),
            ("1:0.1", "2.0", Ordering::Greater),
            ("1.10", "1.9", Ordering::Greater),
            ("1.0+dfsg-1", "1.0-1", Ordering::Greater),
            ("2.0.0", "2.0", Ordering::Greater),
            ("1.0a", "1.0", Ordering::Greater),
        ];
        for (left, right, expected) in cases {
            assert_eq!(
                debian_version_cmp(left, right),
                expected,
                "{left} vs {right}"
            );
            assert_eq!(debian_version_cmp(right, left), expected.reverse());
        }
    }
}
