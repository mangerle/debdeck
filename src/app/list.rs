//! 列表视图状态：搜索、筛选、排序、选中，以及派生出来的「可见行」。
//!
//! `visible` 存的是 [`crate::app::catalog::Catalog::entries`] 的下标，因此它必须
//! 和条目表的版本配对。这里用 `(revision, search, filter, sort, ascending)` 作为
//! 计算凭据：任何一项变了都会重算，调用方不再需要记得「改完状态要把脏标记置位」，
//! 下标越界的可能也被这个凭据本身挡死。

use std::cmp::Ordering;
use std::time::UNIX_EPOCH;

use crate::model::AppEntry;
use crate::util::{matches_tokens, query_tokens};

use super::catalog::Catalog;
use super::state::{DetailTab, Filter, SortKey};

/// 可见行的计算凭据。
#[derive(PartialEq)]
struct ComputeKey {
    revision: usize,
    search: String,
    filter: Filter,
    sort: SortKey,
    ascending: bool,
}

/// 纯视图状态，不持有任何系统数据。
pub(crate) struct ListView {
    pub(crate) search: String,
    pub(crate) filter: Filter,
    pub(crate) sort: SortKey,
    pub(crate) ascending: bool,
    pub(crate) selected: Option<String>,
    pub(crate) detail_tab: DetailTab,
    pub(crate) files_query: String,
    /// `Catalog::entries` 的下标，按当前筛选与排序排列。
    visible: Vec<usize>,
    computed: Option<ComputeKey>,
}

impl Default for ListView {
    fn default() -> Self {
        Self {
            search: String::new(),
            filter: Filter::Gui,
            sort: SortKey::Name,
            ascending: true,
            selected: None,
            detail_tab: DetailTab::Info,
            files_query: String::new(),
            visible: Vec::new(),
            computed: None,
        }
    }
}

impl ListView {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 当前筛选条件下应显示的行。
    pub(crate) fn visible(&self) -> &[usize] {
        &self.visible
    }

    /// 凭据是否仍然有效。逐字段比较，避免每帧为搜索词分配一份克隆。
    fn is_current(&self, catalog: &Catalog) -> bool {
        self.computed.as_ref().is_some_and(|key| {
            key.revision == catalog.revision()
                && key.search == self.search
                && key.filter == self.filter
                && key.sort == self.sort
                && key.ascending == self.ascending
        })
    }

    /// 需要时重算可见行；返回是否发生了重算。
    pub(crate) fn ensure_current(&mut self, catalog: &Catalog) -> bool {
        if self.is_current(catalog) {
            return false;
        }

        let entries = catalog.entries();
        self.visible = select_visible_entries(
            entries,
            FilterOptions {
                search: &self.search,
                filter: self.filter,
                sort: self.sort,
                ascending: self.ascending,
            },
        );
        self.computed = Some(ComputeKey {
            revision: catalog.revision(),
            search: self.search.clone(),
            filter: self.filter,
            sort: self.sort,
            ascending: self.ascending,
        });

        // 选中项被筛掉时跟随到第一行，避免出现「右侧详情为空、左侧却没有高亮」。
        let selected_still_visible = self
            .selected
            .as_ref()
            .is_some_and(|pkg| visible_contains(&self.visible, entries, pkg));
        if !selected_still_visible {
            let first = self
                .visible
                .first()
                .map(|&index| entries[index].pkg.clone());
            self.select(first);
        }
        true
    }

    /// 改变选中项；选中变化要清掉文件页的过滤词。
    pub(crate) fn select(&mut self, pkg: Option<String>) {
        if self.selected != pkg {
            self.selected = pkg;
            self.files_query.clear();
        }
    }

    pub(crate) fn selected_entry<'a>(&self, catalog: &'a Catalog) -> Option<&'a AppEntry> {
        self.selected.as_deref().and_then(|pkg| catalog.entry(pkg))
    }
}

fn entry_matches(entry: &AppEntry, filter: Filter, tokens: &[String]) -> bool {
    let in_filter = match filter {
        Filter::All => true,
        Filter::Installed => entry.is_installed,
        Filter::Gui => entry.is_gui,
        Filter::Held => entry.is_held,
    };
    in_filter && (tokens.is_empty() || matches_tokens(&entry.search_text, tokens))
}

fn compare_entries(
    entries: &[AppEntry],
    left: usize,
    right: usize,
    options: FilterOptions<'_>,
) -> Ordering {
    let (left, right) = (&entries[left], &entries[right]);
    let order = match options.sort {
        SortKey::Name => left.sort_name.cmp(&right.sort_name),
        SortKey::Size => left.installed_size_kb.cmp(&right.installed_size_kb),
        SortKey::Updated => left
            .updated
            .unwrap_or(UNIX_EPOCH)
            .cmp(&right.updated.unwrap_or(UNIX_EPOCH)),
    };
    let order = if options.ascending {
        order
    } else {
        order.reverse()
    };
    order
        .then_with(|| left.sort_name.cmp(&right.sort_name))
        .then_with(|| left.pkg.cmp(&right.pkg))
}

/// 某个包是否仍在可见行里。
fn visible_contains(visible: &[usize], entries: &[AppEntry], pkg: &str) -> bool {
    visible
        .iter()
        .any(|&index| entries[index].pkg.as_str() == pkg)
}

/// 列表筛选与排序参数。
#[derive(Clone, Copy)]
struct FilterOptions<'a> {
    search: &'a str,
    filter: Filter,
    sort: SortKey,
    ascending: bool,
}

/// 按筛选与排序挑选要显示的行下标。
///
/// 搜索与筛选叠加：条目既要落在当前分类里，也要匹配搜索词（无词时只看分类），
/// 用户选了哪个分类就在哪个分类里搜。
fn select_visible_entries(entries: &[AppEntry], options: FilterOptions<'_>) -> Vec<usize> {
    let tokens = query_tokens(options.search);
    let mut visible: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry_matches(entry, options.filter, &tokens))
        .map(|(index, _)| index)
        .collect();

    visible.sort_by(|&left, &right| compare_entries(entries, left, right, options));
    visible
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DpkgState, InstalledData, InstalledPackage, PackageMetadata};
    use crate::runtime::CatalogUpdate;

    fn entry(pkg: &str, installed: bool, gui: bool, size: u64) -> AppEntry {
        AppEntry {
            pkg: pkg.to_string(),
            display_name: pkg.to_string(),
            is_installed: installed,
            is_gui: gui,
            is_held: false,
            has_update: false,
            installed_size_kb: size,
            updated: None,
            latest_version: None,
            search_text: pkg.to_string(),
            sort_name: pkg.to_string(),
        }
    }

    #[test]
    fn filters_and_sorts_entries() {
        let entries = [entry("z", true, false, 10), entry("a", true, true, 20)];
        let visible = select_visible_entries(
            &entries,
            FilterOptions {
                search: "",
                filter: Filter::Gui,
                sort: SortKey::Name,
                ascending: true,
            },
        );
        assert_eq!(visible, vec![1]);
        let visible = select_visible_entries(
            &entries,
            FilterOptions {
                search: "",
                filter: Filter::All,
                sort: SortKey::Size,
                ascending: false,
            },
        );
        assert_eq!(visible, vec![1, 0]);
        // 搜索与筛选叠加：命中关键词还得落在所选分类里（"z" 不是界面应用）。
        let visible = select_visible_entries(
            &entries,
            FilterOptions {
                search: "z",
                filter: Filter::Gui,
                sort: SortKey::Name,
                ascending: true,
            },
        );
        assert!(visible.is_empty());
        let visible = select_visible_entries(
            &entries,
            FilterOptions {
                search: "z",
                filter: Filter::Installed,
                sort: SortKey::Name,
                ascending: true,
            },
        );
        assert_eq!(visible, vec![0]);
    }

    #[test]
    fn same_size_entries_keep_a_stable_order() {
        let entries = [entry("bbb", true, true, 10), entry("aaa", true, true, 10)];
        let visible = select_visible_entries(
            &entries,
            FilterOptions {
                search: "",
                filter: Filter::Gui,
                sort: SortKey::Size,
                ascending: true,
            },
        );
        assert_eq!(visible, vec![1, 0], "大小相同时按名称升序");
    }

    #[test]
    fn recomputes_only_when_inputs_change() {
        let mut catalog = Catalog::new();
        catalog.apply(CatalogUpdate::default().installed(Ok(InstalledData::default())));
        let mut list = ListView::new();
        list.filter = Filter::All;

        assert!(list.ensure_current(&catalog), "首次必须计算");
        assert!(!list.ensure_current(&catalog), "输入未变不应重算");
        list.search = "zzz".to_string();
        assert!(list.ensure_current(&catalog), "换了搜索词要重算");
    }

    #[test]
    fn selection_follows_when_filtered_out() {
        let mut catalog = Catalog::new();
        let mut data = InstalledData::default();
        for name in ["aaa", "bbb"] {
            data.packages.insert(
                name.to_string(),
                InstalledPackage {
                    metadata: PackageMetadata {
                        name: name.to_string(),
                        version: "1".to_string(),
                        arch: "amd64".to_string(),
                        ..Default::default()
                    },
                    dpkg: DpkgState::parse("install ok installed"),
                    control: crate::deb822::Stanza::default(),
                    files: vec![],
                    desktop: None,
                    updated: None,
                },
            );
        }
        catalog.apply(CatalogUpdate::default().installed(Ok(data)));

        let mut list = ListView::new();
        list.filter = Filter::All;
        list.ensure_current(&catalog);
        list.select(Some("bbb".to_string()));
        assert!(list.selected_entry(&catalog).is_some());

        // 换成只看在界面应用：两个包都不是 GUI，选中项应被清空。
        list.filter = Filter::Gui;
        list.ensure_current(&catalog);
        assert!(list.visible().is_empty());
        assert!(list.selected.is_none());
    }
}
