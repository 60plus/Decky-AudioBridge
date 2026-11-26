//! A small log file in the application data folder. It never holds audio.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

static LOG: Mutex<Option<File>> = Mutex::new(None);
const MAX_BYTES: u64 = 1024 * 1024;

pub fn path() -> PathBuf {
    crate::settings::data_dir().join("receiver.log")
}

pub fn init() {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path)
        .map(|m| m.len() > MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
    }
}

pub fn write(message: &str) {
    let stamp = humantime::format_rfc3339_seconds(SystemTime::now());
    let line = format!("{stamp} {message}\n");
    if let Some(file) = LOG.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = file.write_all(line.as_bytes());
    }
    #[cfg(debug_assertions)]
    eprint!("{line}");
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::logfile::write(&format!($($arg)*)) };
}
