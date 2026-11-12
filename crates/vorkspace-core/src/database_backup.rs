//! Owner-only, verified copies of a database taken before a one-time conversion.
use rusqlite::{Connection, OpenFlags};
use std::{
    fs::{self, DirBuilder, File},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Copy `database` into `<its directory>/backups/<UTC time>-<label>/`. The
/// copy is flushed and must open and pass SQLite's own check before the caller
/// may change anything; any failure is returned so the caller can refuse.
pub(crate) fn write(conn: &Connection, database: &Path, label: &str) -> Result<PathBuf, String> {
    let name = database
        .file_name()
        .ok_or("The database has no file name to back up.")?;
    let directory = database
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let backups = directory.join("backups");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&backups)
        .map_err(|error| format!("Cannot create the backup folder: {error}"))?;
    if !fs::symlink_metadata(&backups).is_ok_and(|meta| meta.is_dir() && !meta.is_symlink()) {
        return Err("The backup folder is not a directory.".into());
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let folder = backups.join(format!("{stamp}-{label}"));
    DirBuilder::new()
        .mode(0o700)
        .create(&folder)
        .map_err(|error| format!("Cannot create the backup folder: {error}"))?;
    let copy = folder.join(name);
    let result = copy_and_verify(conn, &copy).and_then(|()| {
        File::open(&folder)?.sync_all()?;
        File::open(&backups)?.sync_all()?;
        File::open(directory)?.sync_all()
    });
    match result {
        Ok(()) => Ok(folder),
        Err(error) => {
            let _ = fs::remove_dir_all(&folder);
            Err(format!(
                "The database backup failed, so nothing was converted: {error}"
            ))
        }
    }
}

fn copy_and_verify(conn: &Connection, copy: &Path) -> std::io::Result<()> {
    let target = copy
        .to_str()
        .ok_or_else(|| std::io::Error::other("Non-UTF-8 backup path"))?;
    conn.execute("VACUUM INTO ?1", [target])
        .map_err(std::io::Error::other)?;
    fs::set_permissions(copy, fs::Permissions::from_mode(0o600))?;
    File::open(copy)?.sync_all()?;
    let opened = Connection::open_with_flags(copy, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(std::io::Error::other)?;
    let check: String = opened
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(std::io::Error::other)?;
    let objects = |conn: &Connection| {
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(std::io::Error::other)
    };
    if check != "ok" || objects(&opened)? != objects(conn)? {
        return Err(std::io::Error::other("The backup copy did not verify."));
    }
    Ok(())
}
