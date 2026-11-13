//! Publish complete owner-only files in their destination directory.
use std::{
    fs::{self, File},
    io,
    path::Path,
};

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

pub(crate) fn replace(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let directory = parent(path);
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    write(temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    File::open(directory)?.sync_all()
}

pub(crate) fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => File::open(parent(path))?.sync_all(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, os::unix::fs::PermissionsExt};

    #[test]
    fn failed_replacement_leaves_the_previous_file_and_no_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.json");
        fs::write(&path, b"previous").unwrap();
        let failure = replace(&path, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("interrupted write"))
        });
        assert_eq!(failure.unwrap_err().to_string(), "interrupted write");
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);

        replace(&path, |file| file.write_all(b"replacement")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        remove(&path).unwrap();
        remove(&path).unwrap();
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
