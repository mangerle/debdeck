//! 软件源候选版本集合与版本降序不变量维护。

use std::collections::HashMap;

use crate::util::debian_version_cmp;

use super::package::RepoPackage;

/// 软件源里的候选版本集合。
///
/// **不变量**：每个包对应的 `Vec<RepoPackage>` 始终按 Debian 版本号降序、
/// 同版本再按架构降序排列，因此索引 0 就是最新版本。该不变量由唯一的写入口
/// [`Self::record`] 维护，读取方不要再自行排序。
#[derive(Clone, Debug, Default)]
pub struct RepositoryData {
    pub packages: HashMap<String, Vec<RepoPackage>>,
    pub warnings: Vec<String>,
}

impl RepositoryData {
    /// 记录一个候选版本：跳过畸形条目、按（版本, 架构）去重，并维持降序不变量。
    ///
    /// 单个包的版本数通常只有个位数，插入后重排该小数组比在全部解析完再统一排序
    /// 更简单，也让「降序」这件事只有一个出处。
    pub fn record(&mut self, package: RepoPackage) {
        if package.metadata.name.is_empty() || package.metadata.version.is_empty() {
            return;
        }
        let versions = self
            .packages
            .entry(package.metadata.name.clone())
            .or_default();
        if versions.iter().any(|existing| {
            existing.metadata.version == package.metadata.version
                && existing.metadata.arch == package.metadata.arch
        }) {
            return;
        }
        versions.push(package);
        if versions.len() > 1 {
            versions.sort_by(|left, right| {
                debian_version_cmp(&right.metadata.version, &left.metadata.version)
                    .then_with(|| right.metadata.arch.cmp(&left.metadata.arch))
            });
        }
    }

    pub fn versions(&self, pkg: &str) -> &[RepoPackage] {
        self.packages
            .get(pkg)
            .map_or(&[] as &[RepoPackage], Vec::as_slice)
    }

    /// 最新版本（降序不变量的唯一消费者）。
    pub fn latest(&self, pkg: &str) -> Option<&RepoPackage> {
        self.versions(pkg).first()
    }

    /// 合并另一份局部索引扫描结果，保持版本降序与（版本, 架构）去重不变量。
    pub fn merge(&mut self, other: Self) {
        for (name, versions) in other.packages {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.packages.entry(name) {
                slot.insert(versions);
            } else {
                for package in versions {
                    self.record(package);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PackageMetadata;

    fn metadata(name: &str, version: &str) -> PackageMetadata {
        PackageMetadata {
            name: name.to_string(),
            version: version.to_string(),
            arch: crate::model::intern_arch("amd64"),
            summary: "summary".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn keeps_versions_sorted_by_debian_version() {
        let mut repository = RepositoryData::default();
        for (name, version) in [("demo", "1.0"), ("demo", "2.0-1"), ("demo", "1.10~rc1")] {
            repository.record(RepoPackage {
                metadata: metadata(name, version),
                ..Default::default()
            });
        }
        let versions = repository.versions("demo");
        let listed: Vec<&str> = versions
            .iter()
            .map(|package| package.metadata.version.as_str())
            .collect();
        assert_eq!(listed, ["2.0-1", "1.10~rc1", "1.0"]);
        assert_eq!(repository.latest("demo").unwrap().metadata.version, "2.0-1");

        // 重复的（版本, 架构）不会插入第二次。
        repository.record(RepoPackage {
            metadata: metadata("demo", "2.0-1"),
            ..Default::default()
        });
        assert_eq!(repository.versions("demo").len(), 3);
        assert!(repository.latest("absent").is_none());
    }
}
