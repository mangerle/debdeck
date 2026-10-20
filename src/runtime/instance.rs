use std::env;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;

use fs2::FileExt;

pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire() -> Result<Self, String> {
        let mut last_error = None;
        for path in lock_paths() {
            match OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
            {
                Ok(file) => match file.try_lock_exclusive() {
                    Ok(()) => return Ok(Self { _file: file }),
                    Err(error) => {
                        last_error = Some(format!(
                            "无法锁定 {}：{error}；DebDeck 可能已经在运行",
                            path.display()
                        ));
                    }
                },
                Err(error) => {
                    last_error = Some(format!("无法创建锁文件 {}：{error}", path.display()));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "无法获得 DebDeck 单实例锁".to_string()))
    }
}

fn lock_paths() -> Vec<PathBuf> {
    let user = env::var("USER").unwrap_or_else(|_| "default".to_string());
    let mut paths = Vec::new();
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            paths.push(dir.join(format!("debdeck-{user}.lock")));
        }
    }
    paths.push(env::temp_dir().join(format!("debdeck-{user}.lock")));
    paths
}
