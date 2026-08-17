//! Private local filesystem helpers for credential-adjacent BBG state.

use std::{fs, io, path::Path};

/// Create or validate a private directory. On Unix, existing directories are
/// tightened to owner-only permissions; symlinks and non-directories fail.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "private directory is a symlink or unsafe file type: {}",
                        path.display()
                    ),
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "private directory is a symlink or unsafe file type: {}",
                        path.display()
                    ),
                ));
            }
        }
        Err(error) => return Err(error),
    }
    set_dir_permissions(path)
}

/// Reject symlinks and non-regular files before reading sensitive local state.
pub fn validate_regular_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "sensitive path is a symlink or unsafe file type: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Open an existing sensitive file without following a final-component symlink.
/// Validation and permission tightening operate on the opened descriptor, so a
/// pathname replacement cannot redirect the subsequent read.
pub fn open_private_read(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    configure_no_follow(&mut options);
    let file = options.open(path)?;
    validate_open_file(&file)?;
    tighten_open_file(&file)?;
    Ok(file)
}

/// Open a sensitive append-only ledger without following a final-component
/// symlink. Newly created files are owner-only from their first descriptor.
pub fn open_private_append(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    validate_open_file(&file)?;
    tighten_open_file(&file)?;
    Ok(file)
}

fn configure_no_follow(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
}

fn validate_open_file(file: &fs::File) -> io::Result<()> {
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sensitive descriptor is not a regular file",
        ));
    }
    Ok(())
}

fn tighten_open_file(file: &fs::File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Apply owner-read/write permissions to a sensitive file on supported Unix
/// platforms. Other platforms still receive the type and symlink checks.
pub fn set_file_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn set_dir_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "bbg-private-fs-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn creates_private_directory_and_rejects_regular_file() {
        let root = temp("dir");
        ensure_private_dir(&root).unwrap();
        assert!(root.is_dir());
        let file = root.join("file");
        fs::write(&file, b"x").unwrap();
        assert!(ensure_private_dir(&file).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn tightens_permissions_and_rejects_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = temp("mode");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
        ensure_private_dir(&root).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let target = root.join("target");
        fs::write(&target, b"secret").unwrap();
        let link = root.join("link");
        symlink(&target, &link).unwrap();
        assert!(validate_regular_file(&link).is_err());
        assert!(open_private_read(&link).is_err());
        assert!(open_private_append(&link).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
