//! `.desktop` 文件里 `Exec` 字段的分词与字段码清理。
//!
//! freedesktop 规范（Desktop Entry Spec §8.1.1）规定 `Exec` 是一串带引号与
//! 转义的命令行，并可能包含 `%f`、`%U` 之类字段码。启动一个应用前必须：
//!
//! * 按 shell 风格切词（单双引号、反斜杠转义）；
//! * 剥掉字段码 —— 我们没有要传递的文件/URL，保留会让应用收到空参数甚至 `%f` 字面量。

/// 从 token 中移除字段码。`%%` 还原为字面 `%`；已知字段码整对丢弃；
/// 未知的 `%x` 保守地把 x 留下，避免误删真实参数。
fn strip_field_codes(token: &str) -> String {
    let mut result = String::with_capacity(token.len());
    let mut chars = token.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            result.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('%') => {
                chars.next();
                result.push('%');
            }
            Some('f' | 'F' | 'u' | 'U' | 'i' | 'c' | 'k' | 'd' | 'D' | 'n' | 'N' | 'v' | 'm') => {
                chars.next();
            }
            Some(other) => {
                chars.next();
                result.push(other);
            }
            None => {}
        }
    }
    result
}

/// 把 `Exec` 字符串解析成 argv 列表，已去掉字段码与空 token。
pub fn desktop_exec_args(exec: &str) -> Vec<String> {
    let mut args = Vec::with_capacity(exec.split_whitespace().count().max(1));
    let mut current = String::new();
    let mut quoted: Option<char> = None;
    let mut chars = exec.chars().peekable();

    let flush = |current: &mut String, args: &mut Vec<String>| {
        if !current.is_empty() {
            args.push(std::mem::take(current));
        }
    };

    while let Some(ch) = chars.next() {
        match quoted {
            Some(quote) => {
                if ch == quote {
                    quoted = None;
                } else if ch == '\\' && quote == '"' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                '\'' | '"' => quoted = Some(ch),
                ' ' | '\t' | '\n' => flush(&mut current, &mut args),
                _ => current.push(ch),
            },
        }
    }
    flush(&mut current, &mut args);

    args.into_iter()
        .map(|arg| strip_field_codes(&arg))
        .filter(|arg| !arg.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exec_with_quotes_and_field_codes() {
        assert_eq!(
            desktop_exec_args("/usr/bin/app --name \"Hello World\" %U"),
            vec!["/usr/bin/app", "--name", "Hello World"]
        );
        assert_eq!(
            desktop_exec_args("sh -c 'echo hi' %F"),
            vec!["sh", "-c", "echo hi"]
        );
        // `--file=%f` 里的字段码被剥掉，留下参数前缀 —— 与规范一致。
        assert_eq!(
            desktop_exec_args("/usr/bin/app --file=%f"),
            vec!["/usr/bin/app", "--file="]
        );
    }

    #[test]
    fn collapses_unknown_percent_sequences() {
        // 规范只定义 f/F/u/U/i/c/k/d/D/n/N/v/m；未知序列按「丢弃百分号、保留字母」处理，
        // 与本文件既有行为保持一致，避免把裸 %x 传给应用。
        assert_eq!(
            desktop_exec_args("/usr/bin/app --fmt=%Y%%z"),
            vec!["/usr/bin/app", "--fmt=Y%z"]
        );
    }
}
