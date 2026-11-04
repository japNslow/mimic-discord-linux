use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

pub const MAX_JSON: u64 = 32 * 1024 * 1024;
pub const MAX_HISTORY: usize = 100_000;
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}
/// Reject rather than repair external filenames, including cached records.
pub fn safe_relative(raw: &str) -> Option<PathBuf> {
    if raw.is_empty()
        || raw.len() > 240
        || raw.contains(['\\', ':'])
        || raw.chars().any(char::is_control)
        || raw.starts_with('/')
        || raw
            .split('/')
            .any(|v| v.is_empty() || v == "." || v == "..")
    {
        return None;
    }
    let path = Path::new(raw);
    if !path.components().all(|v| matches!(v, Component::Normal(_))) {
        return None;
    }
    Some(path.to_path_buf())
}
pub fn private_dir(path: &Path) -> Result<(), String> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(format!("Cannot create private directory: {e}")),
    }
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err("Unsafe data directory: expected an owned directory with mode 0700".into());
    }
    Ok(())
}
pub fn private_tempdir(parent: &Path, prefix: &str) -> Result<tempfile::TempDir, String> {
    tempfile::Builder::new()
        .prefix(prefix)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)
        .map_err(|e| e.to_string())
}
pub fn data_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .filter(|p| p.is_absolute())
        .ok_or("Set an absolute HOME or XDG_STATE_HOME")?;
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let path = base.join("mimic-linux");
    private_dir(&path)?;
    Ok(path)
}
pub fn read_json(path: &Path, limit: u64) -> Result<String, String> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
        || m.len() > limit
    {
        return Err("Unsafe or oversized data file".into());
    }
    let mut s = String::new();
    f.take(limit + 1)
        .read_to_string(&mut s)
        .map_err(|e| e.to_string())?;
    if s.len() as u64 > limit {
        return Err("Data file exceeds limit".into());
    }
    Ok(s)
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("Missing data directory")?;
    private_dir(dir)?;
    let mut f = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    f.as_file().sync_all().map_err(|e| e.to_string())?;
    f.persist(path).map_err(|e| e.to_string())?;
    File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
pub struct InstanceLock(File);
impl InstanceLock {
    pub fn acquire(dir: &Path) -> Result<Self, String> {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(dir.join("instance.lock"))
            .map_err(|e| e.to_string())?;
        let m = f.metadata().map_err(|e| e.to_string())?;
        if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err("Unsafe instance lock".into());
        }
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Mimic Linux is already running".into());
        }
        Ok(Self(f))
    }
}
impl Drop for InstanceLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_traversal_and_special_paths() {
        for p in [
            "",
            "/tmp/pwn",
            "../x",
            "a/../../x",
            "a/./b",
            "a//b",
            "a/",
            "C:\\x",
            "x\0y",
            "x\ny",
            "a\\b",
        ] {
            assert!(safe_relative(p).is_none(), "{p:?}");
        }
        assert_eq!(
            safe_relative("bin/linux64/factorio").unwrap(),
            PathBuf::from("bin/linux64/factorio")
        );
    }
    #[test]
    fn ids_are_bounded() {
        assert!(valid_id("123456789012345678"));
        for v in ["", "-1", "1/2", "１２３", "123456789012345678901"] {
            assert!(!valid_id(v));
        }
    }
    #[test]
    fn private_files_and_symlink_write() {
        use std::os::unix::fs::symlink;
        let d = private_tempdir(&std::env::temp_dir(), "mimic-test-").unwrap();
        let target = d.path().join("target");
        let file = d.path().join("history.json");
        atomic_write(&target, b"original").unwrap();
        symlink(&target, &file).unwrap();
        assert!(read_json(&file, 50).is_err());
        atomic_write(&file, b"safe").unwrap();
        assert_eq!(read_json(&target, 50).unwrap(), "original");
        assert_eq!(read_json(&file, 50).unwrap(), "safe");
        assert!(read_json(&file, 2).is_err());
    }
    #[test]
    fn rejects_directory_symlink() {
        let d = private_tempdir(&std::env::temp_dir(), "mimic-test-").unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(d.path(), &link).unwrap();
        assert!(private_dir(&link).is_err());
    }
    #[test]
    fn single_instance() {
        let d = private_tempdir(&std::env::temp_dir(), "mimic-test-").unwrap();
        let a = InstanceLock::acquire(d.path()).unwrap();
        assert!(InstanceLock::acquire(d.path()).is_err());
        drop(a);
        assert!(InstanceLock::acquire(d.path()).is_ok());
    }
}
