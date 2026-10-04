use anyhow::{Result, bail};

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        bail!("invalid or unsafe filename");
    }
    Ok(())
}

#[cfg(unix)]
pub fn receive<R: std::io::Read>(
    reader: &mut R,
    base: &std::path::Path,
    name: &str,
    size: u64,
) -> Result<std::path::PathBuf> {
    use crate::protocol::MAX_FILE_SIZE;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

    validate_name(name)?;
    if size > MAX_FILE_SIZE {
        bail!("bridge file exceeds 256 MiB limit");
    }
    fn private_dir(path: &std::path::Path) -> Result<()> {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            bail!(
                "cache directory must be owned by you, mode 0700 and not a symlink: {}",
                path.display()
            );
        }
        Ok(())
    }
    private_dir(base)?;
    let files = base.join("files");
    private_dir(&files)?;
    let request = files.join(uuid::Uuid::new_v4().simple().to_string());
    private_dir(&request)?;
    let result = (|| {
        let temporary = request.join(".partial");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let copied = std::io::copy(&mut std::io::Read::take(&mut *reader, size), &mut file)?;
        if copied != size {
            bail!("transfer interrupted: received {copied} of {size} bytes");
        }
        // Each request owns one stream. EOF commits the transfer, extra bytes reject it.
        let mut extra = [0];
        if reader.read(&mut extra)? != 0 {
            bail!("bridge sent more than declared file size");
        }
        file.flush()?;
        file.sync_all()?;
        drop(file);
        let final_path = request.join(name);
        fs::rename(temporary, &final_path)?;
        Ok(final_path)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&request);
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::{fs, io::Cursor};

    #[test]
    fn atomic_receive_rejects_truncation_traversal_and_oversize() {
        let root =
            std::env::temp_dir().join(format!("agentdrop-transfer-{}", uuid::Uuid::new_v4()));
        let path = receive(&mut Cursor::new(b"abc"), &root, "图 ' 1.png", 3).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"abc");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        for (data, name, size) in [
            (b"a".as_slice(), "short", 2),
            (b"ab", "long", 1),
            (b"", "../evil", 0),
            (b"", "big", crate::protocol::MAX_FILE_SIZE + 1),
        ] {
            assert!(receive(&mut Cursor::new(data), &root, name, size).is_err());
        }
        assert_eq!(fs::read_dir(root.join("files")).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_symlink_cache_and_control_filenames() {
        let root = std::env::temp_dir().join(format!("agentdrop-symlink-{}", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(std::env::temp_dir(), &root).unwrap();
        assert!(receive(&mut Cursor::new(b""), &root, "file", 0).is_err());
        fs::remove_file(root).unwrap();
        for name in ["", "..", "/tmp/a", "a\\b", "a\nb", "a\x1bb"] {
            assert!(validate_name(name).is_err());
        }
    }
}
