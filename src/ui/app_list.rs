//! 应用列表：可排序表头 + 虚拟化的行列表。
//!
//! 一张表有几千行，全部绘制会掉帧；`egui_extras` 的 `body.rows` 只画视口内的行，
//! 所以这里只关心「哪一行被点了」，可见性裁剪由它负责。
//!
//! 每一列必须各用一次 `row.col(..)` 提交，因此「画一行」表现为几个窄小的列函数，
//! 而不是一个几百行的闭包。

use eframe::egui::{self, Align, Label, Layout, Rect, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder, TableRow};

use crate::app::{Catalog, ListView};
use crate::app::{Filter, SortKey};
use crate::model::AppEntry;
use crate::ui::entry_status;
use crate::util::{format_kib, format_time};

const ROW_HEIGHT: f32 = 46.0;
const HEADER_HEIGHT: f32 = 30.0;

/// 列定义：`(排序键, 标题, 最小宽度, 初始宽度)`。`None` 表示该列不可排序；
/// 初始宽度为 0 表示该列吃掉剩余空间。
const COLUMNS: [(Option<SortKey>, &str, f32, f32); 5] = [
    (Some(SortKey::Name), "应用", 220.0, 0.0),
    (None, "版本", 72.0, 94.0),
    (Some(SortKey::Size), "安装大小", 72.0, 86.0),
    (None, "状态", 88.0, 104.0),
    (Some(SortKey::Updated), "更新时间", 120.0, 142.0),
];

/// 一次表格交互的结果。
#[derive(Default)]
struct TableClick {
    sort: Option<SortKey>,
    select: Option<String>,
}

/// 绘制列表；排序与选中直接写回 [`ListView`]，由它的计算凭据负责重算可见行。
pub(crate) fn show(list: &mut ListView, catalog: &Catalog, ui: &mut Ui) {
    if list.visible().is_empty() {
        show_empty_hint(list, ui);
        return;
    }

    let entries = catalog.entries();
    let mut click = TableClick::default();

    let mut table = TableBuilder::new(ui)
        .id_salt("app_table")
        .striped(true)
        .resizable(false)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center));
    for (_, _, at_least, initial) in COLUMNS {
        table = table.column(if initial > 0.0 {
            Column::initial(initial).at_least(at_least)
        } else {
            Column::remainder().at_least(at_least)
        });
    }

    let table = table.header(HEADER_HEIGHT, |mut header| {
        for (key, title, _, _) in COLUMNS {
            header.col(|ui| {
                draw_header_cell(ui, key, title, list, &mut click.sort);
            });
        }
    });

    table.body(|body| {
        body.rows(ROW_HEIGHT, list.visible().len(), |mut row| {
            let entry = &entries[list.visible()[row.index()]];
            row.set_selected(list.selected.as_deref() == Some(entry.pkg.as_str()));
            draw_row(&mut row, entry);
            if row
                .response()
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                click.select = Some(entry.pkg.clone());
            }
        });
    });

    if let Some(key) = click.sort {
        list.apply_sort_click(key);
    }
    if let Some(pkg) = click.select {
        list.select(Some(pkg));
    }
}

/// 表头单元格：可排序的列渲染成按钮，并显示当前方向箭头。
fn draw_header_cell(
    ui: &mut Ui,
    key: Option<SortKey>,
    title: &str,
    list: &ListView,
    clicked_sort: &mut Option<SortKey>,
) {
    let Some(sort_key) = key else {
        ui.label(
            RichText::new(title)
                .small()
                .color(ui.visuals().weak_text_color()),
        );
        return;
    };

    let active = list.sort == sort_key;
    let color = if active {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let button_clicked = ui
        .add(egui::Button::new(RichText::new(title).small().strong().color(color)).frame(false))
        .clicked();
    // 箭头紧贴按钮，用户会把它当成同一个控件，所以它也得能触发排序。
    let arrow_clicked = active && draw_sort_arrow(ui, list.ascending);
    if button_clicked || arrow_clicked {
        *clicked_sort = Some(sort_key);
    }
}

/// 提交一行的各列内容。
fn draw_row(row: &mut TableRow<'_, '_>, entry: &AppEntry) {
    row.col(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add(Label::new(RichText::new(&entry.display_name).strong()).truncate());
            ui.add(
                Label::new(
                    RichText::new(&entry.pkg)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                )
                .truncate(),
            );
        });
    });
    row.col(|ui| {
        ui.add(Label::new(RichText::new(&entry.version).small()).truncate());
    });
    row.col(|ui| {
        ui.add(Label::new(RichText::new(format_kib(entry.installed_size_kb)).small()).truncate());
    });
    row.col(|ui| {
        let (status, color) = entry_status(ui, entry);
        ui.add(Label::new(RichText::new(status).small().color(color)).truncate());
    });
    row.col(|ui| {
        ui.add(
            Label::new(
                RichText::new(format_time(entry.updated))
                    .small()
                    .color(ui.visuals().weak_text_color()),
            )
            .truncate(),
        );
    });
}

/// 用图形绘制排序方向，避免字体缺少箭头字形时显示成方块。
fn draw_sort_arrow(ui: &mut Ui, ascending: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 14.0), Sense::click());
    let triangle = Rect::from_center_size(rect.center(), egui::vec2(7.0, 4.5));
    let rotation = if ascending {
        std::f32::consts::TAU / 2.0
    } else {
        0.0
    };
    ui.painter().add(egui::Shape::rotated_triangle(
        triangle,
        rotation,
        ui.visuals().strong_text_color(),
    ));
    response.clicked()
}

/// 空列表时的解释文案 —— 区分「没搜到」与「被筛掉了」，否则用户无从下手。
fn show_empty_hint(list: &ListView, ui: &mut Ui) {
    // 第三种情况（「全部」筛选下确实没有任何包）不需要额外解释。
    let hint = if !list.search.is_empty() {
        Some("试试切换筛选条件，或清空搜索关键词")
    } else if list.filter == Filter::Gui {
        Some("当前没有界面应用，可切换到“全部应用”查看命令行软件包")
    } else {
        None
    };

    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("没有匹配的应用")
                .size(16.0)
                .color(ui.visuals().weak_text_color()),
        );
        if let Some(hint) = hint {
            ui.add_space(6.0);
            ui.label(
                RichText::new(hint)
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
        }
    });
}
