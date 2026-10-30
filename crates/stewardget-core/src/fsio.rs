//! Platform-specific file I/O: positional writes, sparse allocation and
//! atomic replacement.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

/// Writes all of `buf` at `offset` without touching a shared cursor, so many
/// connections can write into one file handle concurrently.
#[cfg(unix)]
pub(crate) fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

#[cfg(windows)]
pub(crate) fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_write(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Async wrapper: performs the write on the blocking pool and hands the
/// buffer back for reuse.
pub(crate) async fn write_at(file: &Arc<File>, offset: u64, buf: Vec<u8>) -> io::Result<Vec<u8>> {
    let file = Arc::clone(file);
    tokio::task::spawn_blocking(move || write_all_at(&file, &buf, offset).map(|()| buf))
        .await
        .map_err(io::Error::other)?
}

/// Reserves `len` bytes without physically writing them.
///
/// On Windows the file is first flagged sparse; otherwise NTFS zero-fills
/// everything before a write far into the file, which stalls segmented
/// downloads of large files for seconds. Unix file systems are sparse already.
pub(crate) fn preallocate(file: &File, len: u64) -> io::Result<()> {
    #[cfg(windows)]
    set_sparse(file);
    file.set_len(len)
}

#[cfg(windows)]
fn set_sparse(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;

    let mut returned = 0u32;
    // Best effort: FAT/exFAT don't support sparse files and that's fine.
    // SAFETY: valid handle owned by `file`; no in/out buffers are passed.
    unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
    }
}

/// Creates (or truncates) a file only its owner can read: settings and the
/// download list hold access tokens and browser cookies.
fn create_private(path: &Path) -> io::Result<File> {
    let mut options = File::options();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Creates a data directory that only its owner can enter.
pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes `data` to a temp file next to `path`, then renames it into place.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    {
        use std::io::Write;
        let mut f = create_private(&tmp)?;
        f.write_all(data)?;
        f.sync_data()?;
    }
    rename(&tmp, path)
}

/// `std::fs::rename`, retried for a moment on Windows while another program
/// (a virus scanner, the search indexer, a backup tool) has the file open.
/// Defender scans every finished download, so without this a download could
/// reach 100 % and then fail with "Access is denied".
pub(crate) fn rename(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0u64;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if cfg!(windows) && attempt < 20 && transient(&e) => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(25 * attempt.min(8)));
            }
            other => return other,
        }
    }
}

/// Access denied (5), sharing violation (32) or lock violation (33): another
/// process holds the file for now.
fn transient(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// Sets the modification time from an HTTP `Last-Modified` value.
pub(crate) fn set_mtime_from_http(path: &Path, last_modified: &str) {
    if let Ok(time) = httpdate::parse_http_date(last_modified)
        && let Ok(file) = File::options().write(true).open(path)
    {
        let _ = file.set_modified(time);
    }
}

/// Free space available to this process on the volume holding `dir`.
/// `None` when it can't be determined (the download then proceeds).
#[cfg(windows)]
pub(crate) fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut available = 0u64;
    // SAFETY: `wide` is NUL-terminated; the out-pointers are valid or null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(available)
}

#[cfg(unix)]
pub(crate) fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;

    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `st` is a valid out-pointer.
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn free_space(_dir: &Path) -> Option<u64> {
    None
}

/// Whether two paths name the same file: Windows and macOS file systems ignore
/// case, so `Setup.exe` and `setup.exe` collide there.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Moves a file, copying across volumes when a rename isn't possible.
pub(crate) fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match rename(from, to) {
        Ok(()) => Ok(()),
        // Only a different drive needs a copy; other errors (in use, denied)
        // would leave a second copy behind.
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices && from.exists() => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
        Err(e) => Err(e),
    }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn state_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("stewardget-perm-{}", std::process::id()));
        super::create_private_dir(&dir).unwrap();
        let file = dir.join("settings.json");
        super::write_atomic(&file, b"{}").unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
