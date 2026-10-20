//! 顶栏：标题、搜索、筛选、刷新。

use eframe::egui::{self, Align, Layout, RichText, TextEdit, Ui};

use crate::app::ListView;
use crate::app::{Action, Filter};
use crate::util::APP_NAME;

/// 绘制顶栏。
///
/// `refresh_allowed` 由调用方按 [`crate::app::refresh_allowed`] 算出，
/// 于是「按钮禁用」与「按了也不执行」用的是同一个判断。
pub(crate) fn show(list: &mut ListView, refresh_allowed: bool, ui: &mut Ui) -> Vec<Action> {
    let mut actions = Vec::new();

    egui::Panel::top("top_toolbar").show(ui, |ui| {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(APP_NAME).size(21.0).strong());
            ui.label(
                RichText::new("Ubuntu 应用管理器")
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );

            // 右侧控件从右往左排：刷新、筛选、清除、搜索框。
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                actions.extend(draw_refresh_button(refresh_allowed, ui));
                draw_filter(list, ui);
                draw_clear_button(list, ui);
                draw_search_box(list, ui);
            });
        });
        ui.add_space(6.0);
    });

    if refresh_allowed && keyboard_refresh_requested(ui) {
        actions.push(Action::Refresh);
    }
    actions
}

fn draw_refresh_button(refresh_allowed: bool, ui: &mut Ui) -> Option<Action> {
    let refresh = ui
        .add_enabled(refresh_allowed, egui::Button::new("刷新"))
        .on_hover_text("重新读取已安装应用和 APT 软件源（F5）");
    refresh.clicked().then_some(Action::Refresh)
}

/// 有搜索词时筛选下拉不起作用，直接禁用，避免用户以为切换生效了。
fn draw_filter(list: &mut ListView, ui: &mut Ui) {
    let searching = !list.search.is_empty();
    ui.add_enabled_ui(!searching, |ui| {
        egui::ComboBox::from_id_salt("filter_combo")
            .width(132.0)
            .selected_text(if searching {
                "搜索全部应用"
            } else {
                list.filter.label()
            })
            .show_ui(ui, |ui| {
                for filter in Filter::ALL {
                    if ui
                        .selectable_label(list.filter == filter, filter.label())
                        .clicked()
                    {
                        list.filter = filter;
                    }
                }
            });
    });
}

fn draw_clear_button(list: &mut ListView, ui: &mut Ui) {
    if !list.search.is_empty() && ui.small_button("清除").on_hover_text("清空搜索").clicked()
    {
        list.search.clear();
    }
}

fn draw_search_box(list: &mut ListView, ui: &mut Ui) {
    ui.add(
        TextEdit::singleline(&mut list.search)
            .hint_text("搜索应用名、包名或拼音")
            .desired_width(300.0),
    );
}

/// F5 刷新。与按钮共用同一个可用性判断（由调用方传入）。
fn keyboard_refresh_requested(ui: &Ui) -> bool {
    ui.input(|input| input.key_pressed(egui::Key::F5))
}
