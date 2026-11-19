//! 无输入法干扰的本地密码输入框。
//!
//! # 设计原理
//! 1. **禁止弹出中文输入法（IME）**：不向窗口注册 `EntityInputHandler`，仅通过
//!    `FocusHandle` 与 `on_key_down` 接收物理按键字符，避免 Linux 桌面（fcitx/ibus）
//!    在输入管理员密码时意外激活中文候选栏。
//! 2. **清晰的大圆点掩码**：自绘 `7px` 实心圆点替代过小的默认点号，并提供右侧
//!    显隐切换按钮（`Eye` / `EyeOff`），兼顾隐私与核对便利性。

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::*;

/// 密码输入框向宿主发出的事件。
pub(crate) enum PasswordInputEvent {
    /// 用户按下回车键请求提交密码。
    Submit,
}

/// 密码输入框状态，持有文本缓冲、光标位置、选区及焦点句柄。
pub(crate) struct PasswordInputState {
    value: String,
    /// 按字符索引的光标位置（`0..=chars().count()`）。
    cursor: usize,
    /// 是否处于全选状态（`Ctrl+A` 触发，下一次输入或退格将清空旧内容）。
    selected_all: bool,
    /// `true` 时显示大圆点掩码，`false` 时显示明文。
    masked: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<PasswordInputEvent> for PasswordInputState {}

impl Focusable for PasswordInputState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PasswordInputState {
    /// 创建默认开启掩码的密码输入框状态。
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            selected_all: false,
            masked: true,
            focus_handle: cx.focus_handle(),
        }
    }

    /// 读取当前密码明文。
    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    /// 聚焦到输入框。
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    /// 清空已输入内容并重置状态。
    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.value.clear();
        self.value.shrink_to_fit();
        self.cursor = 0;
        self.selected_all = false;
        self.masked = true;
        cx.notify();
    }

    fn toggle_masked(&mut self, cx: &mut Context<Self>) {
        self.masked = !self.masked;
        cx.notify();
    }

    fn char_count(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_index_at(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map_or(self.value.len(), |(idx, _)| idx)
    }

    fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.selected_all {
            self.value.clear();
            self.cursor = 0;
            self.selected_all = false;
        }
        let byte_idx = self.byte_index_at(self.cursor);
        self.value.insert_str(byte_idx, text);
        self.cursor += text.chars().count();
    }

    fn handle_backspace(&mut self) {
        if self.selected_all {
            self.value.clear();
            self.cursor = 0;
            self.selected_all = false;
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index_at(self.cursor - 1);
        let end = self.byte_index_at(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn handle_delete(&mut self) {
        if self.selected_all {
            self.value.clear();
            self.cursor = 0;
            self.selected_all = false;
            return;
        }
        if self.cursor >= self.char_count() {
            return;
        }
        let start = self.byte_index_at(self.cursor);
        let end = self.byte_index_at(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;

        if mods.control && key.eq_ignore_ascii_case("a") {
            self.selected_all = !self.value.is_empty();
            cx.notify();
            return;
        }
        if mods.control && key.eq_ignore_ascii_case("v") {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let sanitized: String = text.chars().filter(|c| !c.is_control()).collect();
                self.insert_text(&sanitized);
                cx.notify();
            }
            return;
        }
        if mods.control || mods.alt || mods.platform {
            return;
        }

        self.handle_editing_key(key, event.keystroke.key_char.as_deref(), window, cx);
    }

    fn handle_editing_key(
        &mut self,
        key: &str,
        key_char: Option<&str>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match key {
            "enter" | "return" => {
                cx.emit(PasswordInputEvent::Submit);
                return;
            }
            "backspace" => self.handle_backspace(),
            "delete" => self.handle_delete(),
            "left" => {
                self.selected_all = false;
                self.cursor = self.cursor.saturating_sub(1);
            }
            "right" => {
                self.selected_all = false;
                self.cursor = (self.cursor + 1).min(self.char_count());
            }
            "home" => {
                self.selected_all = false;
                self.cursor = 0;
            }
            "end" => {
                self.selected_all = false;
                self.cursor = self.char_count();
            }
            _ => {
                let Some(ch) = key_char else {
                    return;
                };
                let filtered: String = ch.chars().filter(|c| !c.is_control()).collect();
                if filtered.is_empty() {
                    return;
                }
                self.insert_text(&filtered);
            }
        }
        cx.notify();
    }
}

impl Render for PasswordInputState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let theme = cx.theme();
        let border_color = if focused { theme.primary } else { theme.border };
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let primary = theme.primary;
        let selection_bg = primary.opacity(0.16);
        let chars: Vec<char> = self.value.chars().collect();

        h_flex()
            .id("install-password-box")
            .w_full()
            .h(px(38.0))
            .px(px(12.0))
            .gap(px(8.0))
            .items_center()
            .rounded(px(8.0))
            .bg(theme.background)
            .border_1()
            .border_color(border_color)
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.selected_all = false;
                    this.focus(window, cx);
                }),
            )
            .on_key_down(cx.listener(Self::handle_key_down))
            .child(render_password_content(
                &chars,
                self.cursor,
                self.selected_all,
                self.masked,
                focused,
                ContentPalette {
                    fg,
                    muted,
                    primary,
                    selection_bg,
                },
            ))
            .child(render_toggle_button(self.masked, cx))
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
struct ContentPalette {
    fg: Hsla,
    muted: Hsla,
    primary: Hsla,
    selection_bg: Hsla,
}

fn render_password_content(
    chars: &[char],
    cursor: usize,
    selected_all: bool,
    masked: bool,
    focused: bool,
    palette: ContentPalette,
) -> AnyElement {
    let mut row = h_flex()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .items_center()
        .overflow_hidden();

    if chars.is_empty() {
        if focused {
            row = row.child(caret(palette.primary));
        }
        return row
            .child(
                div()
                    .text_sm()
                    .text_color(palette.muted)
                    .child("请输入管理员密码"),
            )
            .into_any_element();
    }

    let mut content = h_flex()
        .items_center()
        .gap(px(if masked { 5.0 } else { 0.0 }));
    if selected_all {
        content = content
            .px(px(3.0))
            .rounded(px(4.0))
            .bg(palette.selection_bg);
    }

    for (idx, ch) in chars.iter().enumerate() {
        if focused && !selected_all && idx == cursor {
            content = content.child(caret(palette.primary));
        }
        if masked {
            content = content.child(
                div()
                    .size(px(7.0))
                    .rounded_full()
                    .bg(palette.fg)
                    .flex_shrink_0(),
            );
        } else {
            content = content.child(div().text_sm().text_color(palette.fg).child(ch.to_string()));
        }
    }
    if focused && !selected_all && cursor >= chars.len() {
        content = content.child(caret(palette.primary));
    }

    row.child(content).into_any_element()
}

fn caret(color: Hsla) -> AnyElement {
    div()
        .w(px(1.5))
        .h(px(18.0))
        .bg(color)
        .flex_shrink_0()
        .into_any_element()
}

fn render_toggle_button(masked: bool, cx: &mut Context<PasswordInputState>) -> AnyElement {
    let icon = if masked {
        IconName::Eye
    } else {
        IconName::EyeOff
    };
    Button::new("toggle-password-visibility")
        .icon(Icon::new(icon).small())
        .compact()
        .ghost()
        .on_click(cx.listener(|this, _, _, cx| this.toggle_masked(cx)))
        .into_any_element()
}
