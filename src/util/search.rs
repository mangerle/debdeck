//! 搜索与排序用的文本索引：小写化 + 中文拼音。
//!
//! 用户既可能输入 `firefox`、也可能输入「火狐」或拼音首字母 `hh`，
//! 因此每个应用预生成一个多行 haystack，检索时只做「所有 token 都出现」。

use pinyin::ToPinyin;

/// 为一段文本生成 `(全拼, 首字母)` 两个拼音键；非中文字符按小写原样保留。
pub fn pinyin_keys(text: &str) -> (String, String) {
    let mut flat = String::with_capacity(text.len() * 2);
    let mut initials = String::with_capacity(text.len());
    for ch in text.chars() {
        if let Some(pinyin) = ch.to_pinyin() {
            flat.push_str(pinyin.plain());
            initials.push_str(pinyin.first_letter());
        } else {
            for lower in ch.to_lowercase() {
                flat.push(lower);
                initials.push(lower);
            }
        }
    }
    (flat, initials)
}

/// 构造 haystack 与排序键。
///
/// 返回值第一个元素是换行分隔的索引文本（显示名、包名、摘要，含中文时附带全拼与首字母），
/// 第二个是用于名称排序的小写键（含中文时转为全拼，使中文应用能按拼音顺序排列）。
pub fn build_search_text(display_name: &str, pkg_name: &str, summary: &str) -> (String, String) {
    let pinyin = (!display_name.is_ascii()).then(|| pinyin_keys(display_name));
    let sort_name = match &pinyin {
        Some((flat, _)) => flat.to_lowercase(),
        None => display_name.to_ascii_lowercase(),
    };

    let estimated = display_name.len() + pkg_name.len() + summary.len() + 16;
    let mut search = String::with_capacity(estimated);
    push_lower(&mut search, display_name);
    if !pkg_name.eq_ignore_ascii_case(display_name) {
        search.push('\n');
        push_lower(&mut search, pkg_name);
    }
    if !summary.is_empty() {
        search.push('\n');
        push_lower(&mut search, summary);
    }
    if let Some((pinyin_flat, pinyin_initials)) = &pinyin {
        search.push('\n');
        push_lower(&mut search, pinyin_flat);
        search.push('\n');
        push_lower(&mut search, pinyin_initials);
    }
    (search, sort_name)
}

fn push_lower(target: &mut String, text: &str) {
    if text.is_ascii() {
        for byte in text.bytes() {
            target.push(byte.to_ascii_lowercase() as char);
        }
    } else {
        for ch in text.chars() {
            for lower in ch.to_lowercase() {
                target.push(lower);
            }
        }
    }
}

/// 把查询串拆成小写 token；空串得到空列表，调用方据此区分「无搜索」与「搜索」。
pub fn query_tokens(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split_whitespace()
        .map(ToString::to_string)
        .collect()
}

/// 全部 token 都命中才算匹配（AND 语义）。
pub fn matches_tokens(haystack: &str, tokens: &[String]) -> bool {
    tokens.iter().all(|token| haystack.contains(token.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_pinyin_search_keys() {
        let (search, sort) = build_search_text("应用商店", "spark-store", "应用商店");
        assert!(search.contains("yingyongshangdian"));
        assert!(search.contains("yysd"));
        assert_eq!(sort, "yingyongshangdian");
    }

    #[test]
    fn requires_all_tokens() {
        assert!(!matches_tokens(
            "火狐 firefox",
            &query_tokens("火 狐 spark")
        ));
        assert!(matches_tokens("火狐 fire fox", &query_tokens("火 fire")));
    }
}
