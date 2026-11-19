//! 可升级软件包集合。
//!
//! # 为什么用 `apt list --upgradable` 而不是本地比较版本
//!
//! 只有 APT 自己知道版本锁定（hold）、pin 优先级、以及依赖冲突下真正会升级的包。
//! 本地 `debian_version_cmp` 会把已保持版本的包也算成「可更新」，语义与 APT 不一致。
//! 因此这里把 APT 的答案当作**权威**；拿不到时由 `model::build_entries` 退回本地
//! 版本比较，并在那条路径上明确标注是降级判定。
//!
//! `LC_ALL=C` 是必需的：输出里的 `[upgradable from:` 是英文标记，本地化后会失配。

use std::collections::HashSet;
use std::process::Command;
use std::time::Duration;

use crate::backend::{BackendError, Result};

use crate::util::run_command;

/// 询问 APT 哪些包可以升级。
///
/// 失败（未装 apt、索引过期、锁被占用）时返回 `Err`，由上层决定是否退回
/// 本地版本比较，不静默吞掉。
pub fn list_upgradable() -> Result<HashSet<String>> {
    let mut command = Command::new("apt");
    command.args(["list", "--upgradable"]).env("LC_ALL", "C");
    let output = run_command(&mut command, Duration::from_secs(120))
        .map_err(|source| BackendError::command("查询可升级软件包", source))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BackendError::CommandFailed {
            operation: "查询可升级软件包",
            code: output.status.code().unwrap_or(-1),
            detail: stderr.trim().to_string(),
        });
    }
    Ok(parse_upgradable_output(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// 解析 `apt list --upgradable` 的标准行：`名字/套件 版本 架构 [upgradable from: 旧版本]`。
fn parse_upgradable_output(stdout: &str) -> HashSet<String> {
    stdout
        .lines()
        .filter(|line| line.contains("[upgradable from:"))
        .filter_map(|line| line.split('/').next())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_upgradable_output() {
        let output = "Listing...\nhello/resolute-updates 2.10-6 amd64 [upgradable from: 2.10-5]\nglibc/resolute 2.39-0ubuntu8 amd64 [upgradable from: 2.39-0ubuntu7]\nN: There are additional security updates available\n";
        let result = parse_upgradable_output(output);
        assert_eq!(result.len(), 2);
        assert!(result.contains("hello"));
        assert!(result.contains("glibc"));
        assert!(!result.contains("resolute-updates"));
    }

    #[test]
    fn ignores_empty_and_unmarked_lines() {
        assert!(
            parse_upgradable_output("Listing...\n\nfoo/resolute 1 amd64 (upgradable)\n").is_empty()
        );
    }
}
