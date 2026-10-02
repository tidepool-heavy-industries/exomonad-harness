//! Private, atomic native-credential storage. Locks are separate from replaceable records.
use super::LoginError;
use fs2::FileExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_RECORD: u64 = 128 * 1024;

pub(super) fn prepare_parent(path: &Path) -> Result<(), LoginError> {
    if !path.is_absolute() {
        return Err(LoginError::Storage);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(LoginError::Storage)?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(parent).map_err(|_| LoginError::Storage)?;
    let metadata = fs::symlink_metadata(parent).map_err(|_| LoginError::Storage)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
    {
        return Err(LoginError::Storage);
    }
    Ok(())
}

fn check(file: &File) -> Result<(), LoginError> {
    let m = file.metadata().map_err(|_| LoginError::Storage)?;
    // Same-user files only; reject links/devices and permissions exposing credentials.
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
        || m.nlink() != 1
    {
        return Err(LoginError::Storage);
    }
    Ok(())
}

pub(super) fn read(path: &Path) -> Result<Option<Vec<u8>>, LoginError> {
    if !path.is_absolute() {
        return Err(LoginError::Storage);
    }
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path);
    let mut file = match opened {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(LoginError::Storage),
    };
    check(&file)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_RECORD + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LoginError::Storage)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(LoginError::Storage);
    }
    Ok(Some(bytes))
}

pub(super) fn lock(path: &Path) -> Result<File, LoginError> {
    prepare_parent(path)?;
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(PathBuf::from(name))
        .map_err(|_| LoginError::Storage)?;
    check(&file)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25))
            }
            Err(_) => return Err(LoginError::Storage),
        }
    }
}

pub(super) fn write(path: &Path, bytes: &[u8]) -> Result<(), LoginError> {
    if bytes.len() as u64 > MAX_RECORD {
        return Err(LoginError::Storage);
    }
    prepare_parent(path)?;
    let parent = path.parent().ok_or(LoginError::Storage)?;
    let temp = parent.join(format!(".chatgpt-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temp)
            .map_err(|_| LoginError::Storage)?;
        f.write_all(bytes)
            .and_then(|_| f.sync_all())
            .map_err(|_| LoginError::Storage)?;
        // Refuse an existing unsafe destination instead of replacing an unexpected file.
        read(path)?;
        fs::rename(&temp, path).map_err(|_| LoginError::Storage)?;
        File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|_| LoginError::Storage)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
