//! Replacing a file with a newly written one, keeping who may access it (#7).
//!
//! Saving writes a temporary file next to the destination and moves it into place, so
//! that a failed write never leaves the destination truncated. A moved file has the
//! temporary file's access settings, though, so those of the file it replaces are
//! carried over first, with each system's own means:
//!
//! - Windows: `ReplaceFileW`, which keeps the replaced file's access control list
//!   (DACL), attributes, creation time and other streams. Like other programs saving
//!   a file, the user who saves becomes its owner.
//! - macOS: the mode, owner and group, and the access control list and extended
//!   attributes (`copyfile` with `COPYFILE_ACL | COPYFILE_XATTR`).
//! - Linux: the mode, owner and group, and the extended attributes, which hold POSIX
//!   access control lists (`system.posix_acl_access`).
//!
//! When the owner, group or access control list cannot be carried over (e.g. the file
//! belongs to another user), the destination is left as it is and saving fails.

use std::path::Path;

/// Move `temporary` to `destination`, replacing it if it exists (see the module
/// documentation). On failure, `destination` is unchanged and `temporary` is left for
/// the caller to remove. On success, a warning for the user when something besides
/// the saving went wrong (the previous version of the file could not be removed).
///
/// A symbolic link at `destination` is replaced by the file, as a new file in its
/// place (callers that mean to replace the file a link points to pass that file).
pub fn replace_file(temporary: &Path, destination: &Path) -> Result<Option<String>, String> {
    let rename = || {
        std::fs::rename(temporary, destination)
            .map(|()| None)
            .map_err(|error| error.to_string())
    };
    match std::fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => rename(),
        Ok(_) => replace_existing(temporary, destination),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => rename(),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(windows)]
fn replace_existing(temporary: &Path, destination: &Path) -> Result<Option<String>, String> {
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    // A backup name, so that a failed replacement never leaves the replaced file
    // deleted: without one, ReplaceFileW can fail after deleting it
    // (ERROR_UNABLE_TO_MOVE_REPLACEMENT).
    let backup = unused_name(destination, "bak");
    let (replaced, replacement, backup_name) = (
        verbatim(destination)?,
        verbatim(temporary)?,
        verbatim(&backup)?,
    );
    // No flags: failing to keep the replaced file's access control list or attributes
    // fails the replacement (REPLACEFILE_IGNORE_MERGE_ERRORS would ignore that).
    // SAFETY: the paths are NUL-terminated wide strings that outlive the call, and the
    // reserved arguments are null.
    let replaced_ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            backup_name.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced_ok == 0 {
        let error = std::io::Error::last_os_error();
        // The replaced file was moved to the backup but the new one not moved into
        // place (ERROR_UNABLE_TO_MOVE_REPLACEMENT_2): put the replaced file back, or
        // say where it is.
        if !destination.exists() && backup.exists() {
            if let Err(restore_error) = std::fs::rename(&backup, destination) {
                return Err(format!(
                    "Could not replace {}: {error}. The original file could not be moved back ({restore_error}) and is now {}.",
                    destination.display(),
                    backup.display()
                ));
            }
        }
        // E.g. ReplaceFileW opens the replaced file to read it (to keep its access
        // settings and streams), which a file that may be deleted but not read refuses
        // (#32); its access settings cannot be kept then, so it is not replaced (#7).
        // Access can also be denied to the new file, so deleting the existing one
        // first would not help (and would lose it): only another name is suggested.
        if error.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
            return Err(format!(
                "Cannot save over {}: Windows did not allow replacing it while keeping who may access it ({error}). Save it under another name instead.",
                destination.display()
            ));
        }
        return Err(format!(
            "Could not replace {}: {error}",
            destination.display()
        ));
    }
    // The replaced file, kept only until the replacement succeeded. It must not stay
    // behind (saving may be meant to remove patient data), so removing it is retried
    // (e.g. while a virus scanner has it open) and a failure is reported: as a warning,
    // the file having been saved (#32).
    let mut removed = std::fs::remove_file(&backup);
    for _ in 0..10 {
        if removed.is_ok() || !backup.exists() {
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        removed = std::fs::remove_file(&backup);
    }
    match removed {
        Ok(()) => Ok(None),
        Err(error) => Ok(Some(format!(
            "{} was saved, but the previous version could not be removed ({error}) and is still in {}. Delete it if it should not be kept.",
            destination.display(),
            backup.display()
        ))),
    }
}

#[cfg(windows)]
const ERROR_ACCESS_DENIED: i32 = windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32;

/// A path next to `path` that does not exist yet, ending in `extension`.
#[cfg(windows)]
fn unused_name(path: &Path, extension: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let unique = format!(
            ".taglet-{}-{}.{extension}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let candidate = path.with_file_name(unique);
        if !candidate.exists() {
            return candidate;
        }
    }
}

/// `path` as a NUL-terminated wide string in the extended-length form (`\\?\C:\...`,
/// `\\?\UNC\server\share\...`), which is not limited to MAX_PATH characters. The
/// standard library does this for its own file functions.
#[cfg(windows)]
fn verbatim(path: &Path) -> Result<Vec<u16>, String> {
    use std::os::windows::ffi::OsStrExt;

    // Absolute and normalized (no `.` or `..`, backslashes), as extended-length paths
    // must be.
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let wide = absolute.as_os_str().encode_wide().collect::<Vec<u16>>();
    let text = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    let mut out = if wide.starts_with(&text(r"\\?\")) || wide.starts_with(&text(r"\\.\")) {
        wide
    } else if let Some(unc) = wide.strip_prefix(text(r"\\").as_slice()) {
        [text(r"\\?\UNC\").as_slice(), unc].concat()
    } else {
        [text(r"\\?\").as_slice(), &wide].concat()
    };
    out.push(0);
    Ok(out)
}

#[cfg(unix)]
fn replace_existing(temporary: &Path, destination: &Path) -> Result<Option<String>, String> {
    use std::os::unix::fs::MetadataExt;

    let existing = std::fs::metadata(destination).map_err(|error| error.to_string())?;
    let written = std::fs::metadata(temporary).map_err(|error| error.to_string())?;
    if (written.uid(), written.gid()) != (existing.uid(), existing.gid()) {
        std::os::unix::fs::chown(temporary, Some(existing.uid()), Some(existing.gid())).map_err(
            |error| {
                format!(
                    "Cannot save over {}: its owner or group cannot be kept ({error}). Save it under another name instead.",
                    destination.display()
                )
            },
        )?;
    }
    // After the owner, which can clear set-user-ID and set-group-ID bits.
    std::fs::set_permissions(temporary, existing.permissions())
        .map_err(|error| error.to_string())?;
    copy_access_control(destination, temporary).map_err(|error| {
        format!(
            "Cannot save over {}: its access control list or extended attributes cannot be kept ({error}). Save it under another name instead.",
            destination.display()
        )
    })?;
    std::fs::rename(temporary, destination)
        .map(|()| None)
        .map_err(|error| error.to_string())
}

/// Copy the access control list and extended attributes of `from` to `to`.
#[cfg(target_os = "macos")]
fn copy_access_control(from: &Path, to: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path =
        |path: &Path| CString::new(path.as_os_str().as_bytes()).map_err(|error| error.to_string());
    let (from, to) = (path(from)?, path(to)?);
    // SAFETY: both paths are NUL-terminated C strings that outlive the call, and no
    // copyfile state is passed.
    let result = unsafe {
        libc::copyfile(
            from.as_ptr(),
            to.as_ptr(),
            std::ptr::null_mut(),
            libc::COPYFILE_ACL | libc::COPYFILE_XATTR,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// Copy the extended attributes of `from` to `to`, including the POSIX access control
/// list (`system.posix_acl_access`). Other attributes that cannot be read or set (e.g.
/// `security.selinux` without the right) are skipped: they do not widen who may access
/// the file.
#[cfg(all(unix, not(target_os = "macos")))]
fn copy_access_control(from: &Path, to: &Path) -> Result<(), String> {
    const POSIX_ACL: &str = "system.posix_acl_access";
    let names = match xattr::list(from) {
        Ok(names) => names,
        // File systems without extended attributes have no access control lists.
        // On Linux, ENOTSUP is EOPNOTSUPP.
        Err(error) if error.raw_os_error() == Some(libc::ENOTSUP) => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    for name in names {
        // Only the access control list must be kept; other attributes are kept when
        // they can be read and written (e.g. `user.*` ones need read permission).
        let copied = xattr::get(from, &name).and_then(|value| match value {
            Some(value) => xattr::set(to, &name, &value),
            None => Ok(()),
        });
        if let Err(error) = copied {
            if name == POSIX_ACL {
                return Err(error.to_string());
            }
        }
    }
    // A temporary file in a folder with a default access control list inherits one,
    // which the replaced file, without one, did not have (its mode says it all).
    if xattr::get(from, POSIX_ACL)
        .map(|value| value.is_none())
        .unwrap_or(false)
    {
        match xattr::get(to, POSIX_ACL) {
            Ok(Some(_)) => xattr::remove(to, POSIX_ACL).map_err(|error| error.to_string())?,
            Ok(None) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("taglet-replace-{}-{name}", std::process::id()))
    }

    #[test]
    fn replaces_and_creates_files() {
        let destination = temp_path("destination.dcm");
        let temporary = temp_path("temporary.dcm");
        std::fs::write(&temporary, b"new").unwrap();
        replace_file(&temporary, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"new");
        std::fs::write(&temporary, b"newer").unwrap();
        replace_file(&temporary, &destination).unwrap();
        let content = std::fs::read(&destination).unwrap();
        let temporary_left = temporary.exists();
        std::fs::remove_file(&destination).ok();

        assert_eq!(content, b"newer");
        assert!(!temporary_left);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        let destination = temp_path("mode.dcm");
        let temporary = temp_path("mode-temporary.dcm");
        std::fs::write(&destination, b"old").unwrap();
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::fs::write(&temporary, b"new").unwrap();
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600)).unwrap();
        let replaced = replace_file(&temporary, &destination);
        let mode = std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .mode();
        std::fs::remove_file(&destination).ok();

        replaced.unwrap();
        assert_eq!(mode & 0o777, 0o640);
    }

    /// Extended attributes, which hold POSIX access control lists, are kept.
    #[cfg(target_os = "linux")]
    #[test]
    fn keeps_extended_attributes() {
        let destination = temp_path("xattr.dcm");
        let temporary = temp_path("xattr-temporary.dcm");
        std::fs::write(&destination, b"old").unwrap();
        if xattr::set(&destination, "user.taglet", b"kept").is_err() {
            // The file system does not support user attributes.
            std::fs::remove_file(&destination).ok();
            eprintln!("skipped: no user extended attributes");
            return;
        }
        std::fs::write(&temporary, b"new").unwrap();
        let replaced = replace_file(&temporary, &destination);
        let kept = xattr::get(&destination, "user.taglet").unwrap();
        std::fs::remove_file(&destination).ok();

        replaced.unwrap();
        assert_eq!(kept.as_deref(), Some(&b"kept"[..]));
    }

    /// A POSIX access control list (here giving one more user read access) is kept.
    #[cfg(target_os = "linux")]
    #[test]
    fn keeps_posix_access_control_lists() {
        // ACL_EA_VERSION, then entries of tag, permissions and id: the owner (rw),
        // user 65534 (r), the owning group (r), the mask (r) and others (none).
        let mut acl = 2_u32.to_le_bytes().to_vec();
        for (tag, permissions, id) in [
            (0x01_u16, 6_u16, u32::MAX),
            (0x02, 4, 65534),
            (0x04, 4, u32::MAX),
            (0x10, 4, u32::MAX),
            (0x20, 0, u32::MAX),
        ] {
            acl.extend_from_slice(&tag.to_le_bytes());
            acl.extend_from_slice(&permissions.to_le_bytes());
            acl.extend_from_slice(&id.to_le_bytes());
        }
        let destination = temp_path("acl.dcm");
        let temporary = temp_path("acl-temporary.dcm");
        std::fs::write(&destination, b"old").unwrap();
        if xattr::set(&destination, "system.posix_acl_access", &acl).is_err() {
            // The file system does not support POSIX access control lists.
            std::fs::remove_file(&destination).ok();
            eprintln!("skipped: no POSIX ACL support");
            return;
        }
        std::fs::write(&temporary, b"new").unwrap();
        let replaced = replace_file(&temporary, &destination);
        let kept = xattr::get(&destination, "system.posix_acl_access").unwrap();
        std::fs::remove_file(&destination).ok();

        replaced.unwrap();
        assert_eq!(kept, Some(acl));
    }

    /// `ReplaceFileW` keeps the replaced file's creation time (with its access control
    /// list and attributes), which a plain rename would not.
    #[cfg(windows)]
    #[test]
    fn keeps_the_creation_time() {
        use std::os::windows::fs::FileTimesExt;
        let destination = temp_path("created.dcm");
        let temporary = temp_path("created-temporary.dcm");
        std::fs::write(&destination, b"old").unwrap();
        let created = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&destination)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_created(created))
            .unwrap();
        std::fs::write(&temporary, b"new").unwrap();
        let replaced = replace_file(&temporary, &destination);
        let kept = std::fs::metadata(&destination).unwrap().created().unwrap();
        let content = std::fs::read(&destination).unwrap();
        std::fs::remove_file(&destination).ok();

        replaced.unwrap();
        assert_eq!(content, b"new");
        assert_eq!(kept, created);
    }

    /// Paths longer than MAX_PATH (260 characters) are replaced too, as the standard
    /// library's own file functions handle them.
    #[cfg(windows)]
    #[test]
    fn replaces_files_with_long_paths() {
        let mut directory =
            std::env::temp_dir().join(format!("taglet-long-{}", std::process::id()));
        let root = directory.clone();
        while directory.as_os_str().len() < 300 {
            directory.push("a-long-folder-name-to-pass-max-path");
        }
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("destination.dcm");
        let temporary = directory.join("temporary.dcm");
        std::fs::write(&destination, b"old").unwrap();
        std::fs::write(&temporary, b"new").unwrap();
        let replaced = replace_file(&temporary, &destination);
        let content = std::fs::read(&destination).unwrap();
        std::fs::remove_dir_all(&root).ok();

        replaced.unwrap();
        assert_eq!(content, b"new");
    }

    /// A temporary file in a folder with a default access control list inherits one;
    /// when the replaced file has none, the saved file has none either.
    #[cfg(target_os = "linux")]
    #[test]
    fn drops_access_control_lists_the_replaced_file_did_not_have() {
        let mut acl = 2_u32.to_le_bytes().to_vec();
        for (tag, permissions, id) in [
            (0x01_u16, 6_u16, u32::MAX),
            (0x02, 4, 65534),
            (0x04, 4, u32::MAX),
            (0x10, 4, u32::MAX),
            (0x20, 0, u32::MAX),
        ] {
            acl.extend_from_slice(&tag.to_le_bytes());
            acl.extend_from_slice(&permissions.to_le_bytes());
            acl.extend_from_slice(&id.to_le_bytes());
        }
        let destination = temp_path("no-acl.dcm");
        let temporary = temp_path("no-acl-temporary.dcm");
        std::fs::write(&destination, b"old").unwrap();
        std::fs::write(&temporary, b"new").unwrap();
        // As if inherited from the folder's default access control list.
        if xattr::set(&temporary, "system.posix_acl_access", &acl).is_err() {
            std::fs::remove_file(&destination).ok();
            std::fs::remove_file(&temporary).ok();
            eprintln!("skipped: no POSIX ACL support");
            return;
        }
        let replaced = replace_file(&temporary, &destination);
        let left = xattr::get(&destination, "system.posix_acl_access").unwrap();
        std::fs::remove_file(&destination).ok();

        replaced.unwrap();
        assert_eq!(left, None);
    }
}
