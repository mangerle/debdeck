//! 详情页「文件」标签页的状态管理。
//!
//! 将原先散落在 `DebDeckApp` 上的 6 个文件相关字段收拢到单一结构体中，
//! 统一维护「当前包切换 → 按需向后台请求文件列表 → 关键词过滤快照」的状态机。

use std::sync::Arc;

use gpui_kit::SharedString;

use crate::runtime::TaskManager;

use super::catalog::Catalog;

/// 详情页文件清单的缓存与过滤状态。
pub(crate) struct DetailFilesState {
    /// 最近一次同步对应的选中包名，用于感知选中切换。
    active_pkg: Option<String>,
    /// 当前选中包按需加载的完整文件清单：`(Catalog 版本, 包名, 文件列表)`。
    loaded: Option<(usize, String, Vec<String>)>,
    /// 已发起异步读取的凭据，避免同版本同包重复提交。
    requested_key: Option<(usize, String)>,
    /// 当前详情页的文件快照（已按过滤词筛选）。
    filtered: Arc<Vec<SharedString>>,
    /// 文件快照的凭据：`(Catalog 版本, 包名, 过滤词)`。
    filtered_key: Option<(usize, String, String)>,
    /// 未过滤的文件总数，供「共 N 个文件」显示。
    total: usize,
    /// 选中项在无窗口上下文中发生变化时，标记需要清空文件过滤输入框。
    pending_input_clear: bool,
}

impl Default for DetailFilesState {
    fn default() -> Self {
        Self {
            active_pkg: None,
            loaded: None,
            requested_key: None,
            filtered: Arc::new(Vec::new()),
            filtered_key: None,
            total: 0,
            pending_input_clear: false,
        }
    }
}

impl DetailFilesState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 过滤后的文件列表快照（O(1) 共享引用）。
    pub(crate) fn files(&self) -> &Arc<Vec<SharedString>> {
        &self.filtered
    }

    /// 未过滤的文件总数。
    pub(crate) fn total(&self) -> usize {
        self.total
    }

    /// 显式切换选中包（已有窗口上下文立即清空输入框时调用）。
    pub(crate) fn reset_for_selection(&mut self, selected: Option<&str>) {
        self.active_pkg = selected.map(ToString::to_string);
        self.filtered_key = None;
        self.pending_input_clear = false;
    }

    /// 若后台或无窗口路径触发了选中变更，记录需要清空文件输入框。
    pub(crate) fn note_selection_if_changed(&mut self, selected: Option<&str>) -> bool {
        if self.active_pkg.as_deref() == selected {
            return false;
        }
        self.active_pkg = selected.map(ToString::to_string);
        self.filtered_key = None;
        self.pending_input_clear = true;
        true
    }

    /// 取出并清除「需要清空文件过滤输入框」标志。
    pub(crate) fn take_pending_input_clear(&mut self) -> bool {
        std::mem::replace(&mut self.pending_input_clear, false)
    }

    /// 写入后台返回的文件清单并失效旧过滤快照。
    pub(crate) fn on_loaded(&mut self, revision: usize, pkg: String, files: Vec<String>) {
        self.loaded = Some((revision, pkg, files));
        self.filtered_key = None;
    }

    /// 清空过滤关键字后使旧过滤快照失效。
    pub(crate) fn invalidate_filter(&mut self) {
        self.filtered_key = None;
        self.pending_input_clear = false;
    }

    /// 根据当前目录、选中包与过滤词同步文件视图，并在缺失时请求后台加载。
    pub(crate) fn sync(
        &mut self,
        catalog: &Catalog,
        selected: Option<&str>,
        query: &str,
        tasks: &TaskManager,
    ) {
        let Some(pkg_name) = selected else {
            self.clear_view();
            return;
        };
        let Some(package) = catalog.installed_package(pkg_name) else {
            self.clear_view();
            return;
        };

        let revision = catalog.revision();
        let pkg = pkg_name.to_string();
        let arch = package.metadata.arch.to_string();

        let Some((loaded_rev, loaded_pkg, loaded_list)) = self.loaded.as_ref() else {
            self.clear_view();
            self.request_if_needed(revision, pkg, arch, tasks);
            return;
        };
        if *loaded_rev != revision || loaded_pkg != &pkg {
            self.clear_view();
            self.request_if_needed(revision, pkg, arch, tasks);
            return;
        }

        self.total = loaded_list.len();
        let normalized = query.trim().to_lowercase();
        let key = (revision, pkg, normalized.clone());
        if self.filtered_key.as_ref() == Some(&key) {
            return;
        }

        let matched: Vec<SharedString> = if normalized.is_empty() {
            loaded_list.iter().cloned().map(Into::into).collect()
        } else {
            loaded_list
                .iter()
                .filter(|path| path.to_lowercase().contains(&normalized))
                .cloned()
                .map(Into::into)
                .collect()
        };
        self.filtered = Arc::new(matched);
        self.filtered_key = Some(key);
    }

    fn clear_view(&mut self) {
        self.filtered = Arc::new(Vec::new());
        self.filtered_key = None;
        self.total = 0;
    }

    fn request_if_needed(
        &mut self,
        revision: usize,
        pkg: String,
        arch: String,
        tasks: &TaskManager,
    ) {
        let req_key = (revision, pkg.clone());
        if self.requested_key.as_ref() == Some(&req_key) {
            return;
        }
        if tasks.load_package_files(revision, pkg, arch) {
            self.requested_key = Some(req_key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_change_marks_pending_input_clear() {
        let mut state = DetailFilesState::new();
        assert!(state.note_selection_if_changed(Some("bash")));
        assert!(state.take_pending_input_clear());
        assert!(!state.take_pending_input_clear());
        // 相同包名不再重复标记。
        assert!(!state.note_selection_if_changed(Some("bash")));
    }

    #[test]
    fn explicit_reset_clears_pending_flag() {
        let mut state = DetailFilesState::new();
        state.note_selection_if_changed(Some("bash"));
        state.reset_for_selection(Some("vim"));
        assert!(!state.take_pending_input_clear());
    }
}
