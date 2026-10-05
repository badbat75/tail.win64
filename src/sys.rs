//! Thin OS layer. This is the only module allowed to contain `unsafe`.
//!
//! The non-Windows fallbacks exist so the portable logic can be built and
//! tested on Linux too (for example to diff behavior against GNU tail).

use std::fs::File;
use std::io;

/// Identity of an open file: survives renames, changes when a path is
/// re-created (log rotation). Comparable across handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileId {
    volume: u64,
    index: u128,
}

#[cfg(windows)]
mod imp {
    use super::FileId;
    use std::fs::File;
    use std::io;
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, WAIT_TIMEOUT};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FILE_TYPE_DISK, FileIdInfo, GetFileInformationByHandleEx, GetFileType,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

    pub fn file_id(file: &File) -> io::Result<FileId> {
        // FILE_ID_INFO carries the 128-bit id, which is what ReFS needs; the
        // older BY_HANDLE_FILE_INFORMATION index is only unique on NTFS.
        let mut info = MaybeUninit::<FILE_ID_INFO>::uninit();
        // SAFETY: the handle is owned by `file` and valid for the call; the
        // buffer is exactly the size of the requested information class.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                info.as_mut_ptr().cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call succeeded, so the structure is initialized.
        let info = unsafe { info.assume_init() };
        Ok(FileId { volume: info.VolumeSerialNumber, index: u128::from_le_bytes(info.FileId.Identifier) })
    }

    pub fn is_regular_file(file: &File) -> bool {
        // `Metadata::is_file` is also true for pipes on Windows; the handle type is not.
        // SAFETY: the handle is owned by `file` and valid for the call.
        let on_disk = unsafe { GetFileType(file.as_raw_handle()) } == FILE_TYPE_DISK;
        on_disk && file.metadata().is_ok_and(|m| m.is_file())
    }

    pub fn process_alive(pid: u32) -> bool {
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                // Access denied means the process exists but belongs to someone else.
                return GetLastError() == ERROR_ACCESS_DENIED;
            }
            let alive = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
            CloseHandle(handle);
            alive
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::FileId;
    use std::fs::File;
    use std::io;

    pub fn file_id(file: &File) -> io::Result<FileId> {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata()?;
        Ok(FileId { volume: meta.dev(), index: u128::from(meta.ino()) })
    }

    pub fn is_regular_file(file: &File) -> bool {
        file.metadata().is_ok_and(|m| m.is_file())
    }

    pub fn process_alive(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// Returns the identity of an open file.
pub fn file_id(file: &File) -> io::Result<FileId> {
    imp::file_id(file)
}

/// True for a file on disk, i.e. something that can be seeked and scanned backwards.
pub fn is_regular_file(file: &File) -> bool {
    imp::is_regular_file(file)
}

/// True while the process `pid` is running (used by `--pid`).
pub fn process_alive(pid: u32) -> bool {
    imp::process_alive(pid)
}

/// Standard input as a `File` when it is redirected from a regular file
/// (`tail < big.log`), so it can be scanned backwards instead of read in full.
pub fn stdin_regular_file() -> Option<File> {
    #[cfg(windows)]
    let owned = std::os::windows::io::AsHandle::as_handle(&io::stdin()).try_clone_to_owned().ok()?;
    #[cfg(not(windows))]
    let owned = std::os::fd::AsFd::as_fd(&io::stdin()).try_clone_to_owned().ok()?;
    let file = File::from(owned);
    is_regular_file(&file).then_some(file)
}

/// Opens a file for reading without getting in the way of other processes:
/// writers can keep appending and log rotators can rename or delete it.
pub fn open_shared(path: &std::path::Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE};
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    options.open(path)
}
