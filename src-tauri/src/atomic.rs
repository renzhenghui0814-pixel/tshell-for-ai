//! Replacing a file without the chance of losing it.
//!
//! The extension wrote its config by opening the real path and overwriting it.
//! That is fine until the write is interrupted -- a crash, a full disk, the
//! machine losing power -- and then the file on disk is whatever prefix made it
//! out, which parses as nothing and takes every server the user had configured
//! with it. The window is small. The loss is total.
//!
//! Writing a temporary file in the same directory and renaming it over the
//! target closes it. A rename within one filesystem is atomic: any reader sees
//! either the old file or the new one, never a half-written one, and an
//! interrupted write leaves only a stray temporary behind.

use std::fs;
use std::io;
use std::path::Path;

/// Write `contents` to `path`, replacing it in one step.
///
/// The temporary is created beside the target rather than in the system
/// temporary directory, because a rename across filesystems is not a rename --
/// it is a copy and a delete, and it is not atomic.
pub fn write(path: &Path, contents: &str) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    fs::create_dir_all(parent)?;

    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("config");
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));

    // Scoped so the handle is closed before the rename; Windows will not rename
    // a file that is still open.
    fs::write(&temporary, contents)?;

    if let Err(error) = fs::rename(&temporary, path) {
        // Leaving the temporary behind after a failed rename would be litter
        // that never gets collected -- the next attempt writes a new one.
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

/// As [`write`], but the file is readable only by its owner.
///
/// The permission is set on the temporary before it is renamed into place, so
/// the target is never briefly world-readable. On Windows this is [`write`]:
/// the file inherits the directory's ACL, and the directory is under the user's
/// own profile.
pub fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
        })?;
        fs::create_dir_all(parent)?;

        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("config");
        let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));

        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
        }

        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        write(path, contents)
    }
}
