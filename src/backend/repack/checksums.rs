//! 生成 `DEBIAN/md5sums`。
//!
//! 校验和按包内相对路径记录并排序，dpkg 用它检测 conffile 等文件是否被本地修改。
//! MD5 在这里是 Debian 的既有格式要求，不是我们选的摘要算法；下载完整性用的是
//! SHA256（见 [`crate::backend::download`]）。

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result};
use md5::{Digest, Md5};

use crate::util::hex_lower;

use super::{PROGRESS_EVERY, ensure_not_cancelled, set_mode};

/// 为复制进工作目录的文件写 md5sums。
pub fn write_md5sums(
    debian_dir: &Path,
    copied_files: &[(std::path::PathBuf, std::path::PathBuf)],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<()> {
    ensure_not_cancelled(cancel)?;
    progress("正在生成校验和…");

    let mut lines = Vec::with_capacity(copied_files.len());
    for (index, (relative, path)) in copied_files.iter().enumerate() {
        ensure_not_cancelled(cancel)?;
        if index > 0 && index % PROGRESS_EVERY == 0 {
            progress(&format!("正在生成校验和…已处理 {index} 个"));
        }
        let digest =
            md5_of_file(path).with_context(|| format!("无法读取 {} 计算校验和", path.display()))?;
        // Debian 的格式：`<摘要><两个空格><包内相对路径>`。
        lines.push(format!("{digest}  {}", relative.to_string_lossy()));
    }
    lines.sort();

    let path = debian_dir.join("md5sums");
    std::fs::write(&path, format!("{}\n", lines.join("\n")))?;
    set_mode(&path, 0o644)
}

fn md5_of_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Md5::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn writes_sorted_md5_lines() {
        let dir = std::env::temp_dir().join(format!("debdeck-md5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.txt"), b"contents").unwrap();
        std::fs::write(dir.join("a.txt"), b"other").unwrap();

        let copied = vec![
            (PathBuf::from("b.txt"), dir.join("b.txt")),
            (PathBuf::from("a.txt"), dir.join("a.txt")),
        ];
        let cancel = AtomicBool::new(false);
        write_md5sums(&dir, &copied, &cancel, &mut |_| {}).unwrap();

        let text = std::fs::read_to_string(dir.join("md5sums")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        // 按包内路径排序，而不是按传入顺序。
        assert!(lines[0].ends_with("a.txt"), "{lines:?}");
        assert!(lines[1].ends_with("b.txt"));
        // `printf other | md5sum` = 795f3202b17cb6bc3d4b771d8c6c9eaf
        assert!(
            lines[0].starts_with("795f3202b17cb6bc3d4b771d8c6c9eaf  "),
            "第一行={}",
            lines[0]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn respects_cancellation() {
        let dir = std::env::temp_dir().join(format!("debdeck-md5-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cancel = AtomicBool::new(true);
        let error = write_md5sums(&dir, &[], &cancel, &mut |_| {}).unwrap_err();
        assert!(error.to_string().contains("已取消"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
