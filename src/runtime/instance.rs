use std::env;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
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
                    // 已经有人持锁就是「另一个实例在运行」。此时不能再试下一个
                    // 候选路径，否则会各自拿到一把锁、开出两个窗口。
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        return Err(format!("无法锁定 {}：DebDeck 已经在运行", path.display()));
                    }
                    Err(error) => {
                        last_error = Some(format!("无法锁定 {}：{error}", path.display()));
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
    let name = format!("debdeck-{user}.lock");
    let mut paths = Vec::with_capacity(2);
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            paths.push(dir.join(&name));
        }
    }
    paths.push(env::temp_dir().join(name));
    paths
}
