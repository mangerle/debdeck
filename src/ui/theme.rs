//! 配色、字体与排版常量。
//!
//! 界面里所有颜色都应从这里取：状态标签原先在绘制代码里内联了六组明暗 RGB，
//! 换主题色时容易漏改。深色/浅色各准备一版是必要的 —— 只给一个颜色在半深色
//! 背景上会失去对比度。

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Color32, FontData, FontFamily, FontId, TextStyle, Vec2};

/// 主色：Ubuntu 橙。用于强调、选中与主按钮。
pub(crate) const ACCENT: Color32 = Color32::from_rgb(233, 84, 32);
/// 成功态（操作完成）。
pub(crate) const SUCCESS: Color32 = Color32::from_rgb(52, 168, 83);
/// 警告态（可恢复的问题）。
pub(crate) const WARNING: Color32 = Color32::from_rgb(230, 145, 40);
/// 危险态（失败、不可逆操作的确认按钮）。
pub(crate) const DANGER: Color32 = Color32::from_rgb(204, 51, 51);

/// 一对明暗配色，按当前主题选择。
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) dark: Color32,
    pub(crate) light: Color32,
}

/// 「已安装」：绿色系。
pub(crate) const STATUS_INSTALLED: Palette = Palette {
    dark: Color32::from_rgb(129, 199, 132),
    light: Color32::from_rgb(35, 120, 55),
};

/// 「可更新」：橙色系，与主色同族但更亮，避免与选中态混淆。
pub(crate) const STATUS_UPDATE: Palette = Palette {
    dark: Color32::from_rgb(255, 183, 77),
    light: Color32::from_rgb(190, 100, 10),
};

/// 「已保持版本」：蓝色系，与警告/危险区分开 —— 它是用户主动选择的状态，不是问题。
pub(crate) const STATUS_HELD: Palette = Palette {
    dark: Color32::from_rgb(130, 170, 255),
    light: Color32::from_rgb(40, 90, 190),
};

/// 链接色同样需要按明暗主题调整，对比度不足会看不清。
pub(crate) const HYPERLINK: Palette = Palette {
    dark: Color32::from_rgb(255, 160, 115),
    light: Color32::from_rgb(190, 60, 20),
};

/// 加载中文字体并追加到两个字体族末尾。
///
/// egui 自带字体不含 CJK 字形，缺少这一步界面会整片变成方块。
/// 找不到时只记日志、不失败：加载页仍然可读（英文部分），并让字形测试报错。
pub(crate) fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some((path, bytes)) = find_cjk_font() {
        let name = "system-cjk".to_string();
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
        // 追加而不是替换：拉丁字符继续用自带字体，中文走系统中文字体。
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                list.push(name.clone());
            }
        }
        tracing::info!("已加载中文字体：{}", path.display());
    } else {
        tracing::warn!("未找到中文字体，界面中的中文可能无法正常显示");
    }
    ctx.set_fonts(fonts);
}

pub(crate) fn find_cjk_font() -> Option<(PathBuf, Vec<u8>)> {
    let mut candidates: Vec<PathBuf> = [
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-VF.otf.ttc",
        "/usr/share/fonts/truetype/arphic/uming.ttc",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();

    if let Ok(output) = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif:lang=zh-cn"])
        .output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            candidates.push(PathBuf::from(path));
        }
    }

    for path in candidates {
        if !path.is_file() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            return Some((path, bytes));
        }
    }
    None
}

pub(crate) fn configure_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(15.0, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(15.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(22.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        );
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        style.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.35);
        style.visuals.hyperlink_color = if style.visuals.dark_mode {
            HYPERLINK.dark
        } else {
            HYPERLINK.light
        };
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.open.corner_radius = egui::CornerRadius::same(6);
    });
}

/// 界面字体（Ubuntu + 系统中文字体）不含箭头等符号，用错字形会直接显示成方块。
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;

    fn is_cjk(c: char) -> bool {
        matches!(c as u32,
            0x2E80..=0x2EFF | 0x3000..=0x303F | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
            | 0xFF00..=0xFFEF)
    }

    /// 收集源码里**字符串字面量**中出现的非 ASCII、非中日韩文字符号。
    ///
    /// 只认字面量：注释和文档里的箭头、制表符不是界面会渲染的内容，
    /// 把它们算进来会让这个检查长期处于「必须改注释才能变绿」的状态。
    fn collect_symbols(symbols: &mut BTreeSet<char>, dir: &std::path::Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_symbols(symbols, &path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let Ok(source) = fs::read_to_string(&path) else {
                continue;
            };
            symbols.extend(literal_symbols(&source));
        }
    }

    /// 提取字符串字面量里的非常规符号。
    ///
    /// 只做够用即止的识别：跳过 `//` 行注释、按反斜杠跳过转义字符。
    /// 不处理块注释与 `r#"..."#` 原始字符串 —— 这两类里出现的符号只会是文档
    /// 示例，不是界面文本，漏掉不影响这个检查的目的。
    fn literal_symbols(source: &str) -> Vec<char> {
        let mut out = Vec::new();
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                // 行注释：跳到行尾，注释内容不参与字形检查。
                '/' if chars.peek() == Some(&'/') => {
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                '"' => {
                    while let Some(inner) = chars.next() {
                        match inner {
                            '\\' => {
                                // 跳过被转义的下一个字符。
                                chars.next();
                            }
                            '"' => break,
                            c if !c.is_ascii() && !is_cjk(c) && !c.is_whitespace() => out.push(c),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    #[test]
    fn scanner_ignores_comments_but_keeps_literals() {
        // 用有字形的符号做夹具：这个测试要验证的是「注释被跳过」，
        // 而不是引入新的缺字形字符把上面那个测试弄红。
        let source = "// 注释里的 — 不算
/// 文档里的 · 也不算
const S: &str = \"字面量 —\";\n";
        let mut found = literal_symbols(source);
        found.sort_unstable();
        found.dedup();
        assert_eq!(found, vec!['—'], "只应收集字面量里的符号：{found:?}");
    }

    #[test]
    fn every_symbol_used_in_the_ui_has_a_glyph() {
        assert!(
            find_cjk_font().is_some(),
            "本机缺少中文字体，无法验证界面字形覆盖"
        );
        let ctx = egui::Context::default();
        configure_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        output.textures_delta.clear();

        let mut symbols = BTreeSet::new();
        collect_symbols(
            &mut symbols,
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        );

        let missing: Vec<String> = ctx.fonts_mut(|fonts| {
            let font_id = FontId::proportional(15.0);
            symbols
                .iter()
                .filter(|c| !fonts.has_glyph(&font_id, **c))
                .map(|c| format!("{c} (U+{:04X})", *c as u32))
                .collect()
        });
        assert!(
            missing.is_empty(),
            "这些字符在界面字体里没有字形，会显示成方块：{missing:?}"
        );
    }
}
