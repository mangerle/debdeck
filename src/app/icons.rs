//! 应用图标的有界 LRU 缓存。
//!
//! 图标解码在后台线程执行，界面只读取已就绪纹理；缺失图标缓存为 `Ready(None)`，
//! 防止每一帧重绘都重复扫描磁盘主题目录。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use gpui_kit::RenderImage;

use crate::runtime::TaskManager;

/// 图标缓存条目上限：避免长时间浏览后 GPU 纹理只增不减。
const MAX_ICON_CACHE_ENTRIES: usize = 128;

/// 图标缓存条目：区分「后台解码中」与「已得到结果（含缺失）」。
enum IconState {
    Pending,
    Ready(Option<Arc<RenderImage>>),
}

/// 有界 LRU 图标缓存：图标名 → 加载状态；不存在的图标也会缓存为 `Ready(None)`，避免重复磁盘查找。
#[derive(Default)]
pub(crate) struct IconCache {
    entries: HashMap<String, IconState>,
    order: VecDeque<String>,
}

impl IconCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 读取已就绪的图标纹理；尚在后台解码或不存在时返回 `None`。
    pub(crate) fn get_ready(&self, name: &str) -> Option<Arc<RenderImage>> {
        match self.entries.get(name) {
            Some(IconState::Ready(Some(image))) => Some(Arc::clone(image)),
            _ => None,
        }
    }

    /// 若缓存尚未记录该图标，则标记为加载中并向后台提交解码任务；已命中时刷新 LRU 顺序。
    pub(crate) fn request_if_needed(&mut self, name: &str, tasks: &TaskManager, max_size: u32) {
        if name.is_empty() {
            return;
        }
        if self.entries.contains_key(name) {
            self.touch(name);
            return;
        }
        if tasks.load_icon(name.to_string(), max_size) {
            self.insert_entry(name.to_string(), IconState::Pending);
        }
    }

    /// 写入后台返回的解码结果。
    pub(crate) fn insert_loaded(&mut self, name: String, image: Option<Arc<RenderImage>>) {
        self.insert_entry(name, IconState::Ready(image));
    }

    fn insert_entry(&mut self, name: String, state: IconState) {
        if let Some(existing) = self.entries.get_mut(&name) {
            *existing = state;
            self.touch(&name);
            return;
        }
        self.evict_if_full(MAX_ICON_CACHE_ENTRIES);
        self.order.push_back(name.clone());
        self.entries.insert(name, state);
    }

    fn touch(&mut self, name: &str) {
        if let Some(pos) = self.order.iter().position(|item| item == name)
            && let Some(key) = self.order.remove(pos)
        {
            self.order.push_back(key);
        }
    }

    fn evict_if_full(&mut self, capacity: usize) {
        while self.entries.len() >= capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_cache_evicts_least_recently_used() {
        let mut cache = IconCache::new();
        cache.insert_loaded("a".to_string(), None);
        cache.insert_loaded("b".to_string(), None);
        cache.insert_loaded("c".to_string(), None);
        // 访问 a，使其成为最近使用；此时最久未使用的是 b。
        cache.touch("a");
        cache.evict_if_full(3);

        assert!(cache.entries.contains_key("a"));
        assert!(!cache.entries.contains_key("b"));
        assert!(cache.entries.contains_key("c"));
    }
}
