//! Unix descriptor-relative storage. Only these reserved filenames are rotated.
use std::{
    ffi::{CStr, CString},
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::{Component, Path},
};

pub(super) const FILE_BYTES: u64 = 1024 * 1024;
pub(super) const FILE_NAMES: [&CStr; 3] = [c"hang-0.jsonl", c"hang-1.jsonl", c"hang-2.jsonl"];
const LOCK: &CStr = c"hang.lock";

pub(super) struct Log {
    directory: File,
    _lock: File,
    active: Option<File>,
}
impl Log {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let parent = path.parent().ok_or_else(unsafe_path)?;
        let leaf = path.file_name().ok_or_else(unsafe_path)?;
        if leaf != "diagnostics" {
            return Err(unsafe_path());
        }
        let parent = open_directory(parent)?;
        // SAFETY: fixed NUL-terminated name and a live directory descriptor.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), c"diagnostics".as_ptr(), 0o700) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
        let directory = open_at(
            &parent,
            c"diagnostics",
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        )?;
        let metadata = directory.metadata()?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(unsafe_path());
        }
        let lock = open_file(&directory, LOCK)?;
        if lock.metadata()?.len() != 0 {
            return Err(unsafe_path());
        }
        // SAFETY: live file descriptor; nonblocking, process-lifetime exclusive ownership.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut log = Self {
            directory,
            _lock: lock,
            active: None,
        };
        log.validate()?;
        let mut active = open_file(&log.directory, FILE_NAMES[0])?;
        let len = active.metadata()?.len();
        let incomplete = if len > 0 {
            active.seek(SeekFrom::End(-1))?;
            let mut tail = [0];
            active.read_exact(&mut tail)?;
            tail[0] != b'\n'
        } else {
            false
        };
        log.active = Some(active);
        if incomplete {
            log.rotate()?;
        }
        log.directory.sync_all()?;
        Ok(log)
    }

    fn validate(&self) -> io::Result<()> {
        for name in FILE_NAMES {
            match open_at(&self.directory, name, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
                Ok(file) => {
                    validate_file(&file)?;
                    if file.metadata()?.len() > FILE_BYTES {
                        return Err(unsafe_path());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        if let Some(active) = &self.active {
            let named = open_at(
                &self.directory,
                FILE_NAMES[0],
                libc::O_RDONLY | libc::O_NONBLOCK,
                0,
            )?;
            let expected = active.metadata()?;
            let actual = named.metadata()?;
            if expected.dev() != actual.dev() || expected.ino() != actual.ino() {
                return Err(unsafe_path());
            }
        }
        Ok(())
    }

    pub(super) fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > super::MAX_RECORD_BYTES || !bytes.ends_with(b"\n") {
            return Err(unsafe_path());
        }
        self.validate()?;
        let size = self
            .active
            .as_ref()
            .ok_or_else(unsafe_path)?
            .metadata()?
            .len();
        if size + bytes.len() as u64 > FILE_BYTES {
            self.rotate()?;
        }
        let file = self.active.as_mut().ok_or_else(unsafe_path)?;
        file.write_all(bytes)?;
        file.sync_data()
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.validate()?;
        self.active = None;
        // Never traverse links, enumerate/delete user files, or truncate in place.
        // The directory is private, and all reserved entries were checked above.
        unlink(&self.directory, FILE_NAMES[2])?;
        rename(&self.directory, FILE_NAMES[1], FILE_NAMES[2])?;
        rename(&self.directory, FILE_NAMES[0], FILE_NAMES[1])?;
        let active = create_file(&self.directory, FILE_NAMES[0])?;
        validate_file(&active)?;
        self.active = Some(active);
        self.directory.sync_all()
    }

    pub(super) fn flush(&mut self) -> io::Result<()> {
        if let Some(active) = &self.active {
            active.sync_data()?;
        }
        Ok(())
    }
}

fn unsafe_path() -> io::Error {
    io::Error::other("unsafe diagnostic destination")
}

fn open_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(unsafe_path());
    }
    let mut directory = File::open("/")?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                use std::os::unix::ffi::OsStrExt;
                let name = CString::new(name.as_bytes()).map_err(|_| unsafe_path())?;
                directory = open_at(&directory, &name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
            }
            _ => return Err(unsafe_path()),
        }
    }
    Ok(directory)
}
fn validate_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(unsafe_path());
    }
    Ok(())
}
fn open_file(directory: &File, name: &CStr) -> io::Result<File> {
    let file = match create_file(directory, name) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => open_at(
            directory,
            name,
            libc::O_RDWR | libc::O_APPEND | libc::O_NONBLOCK,
            0,
        )?,
        Err(error) => return Err(error),
    };
    validate_file(&file)?;
    Ok(file)
}
fn create_file(directory: &File, name: &CStr) -> io::Result<File> {
    open_at(
        directory,
        name,
        libc::O_RDWR | libc::O_APPEND | libc::O_CREAT | libc::O_EXCL,
        0o600,
    )
}
fn open_at(
    directory: &File,
    name: &CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> io::Result<File> {
    // SAFETY: name is NUL-terminated, directory is live, mode is provided for O_CREAT.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: openat returned a new owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
fn unlink(directory: &File, name: &CStr) -> io::Result<()> {
    // SAFETY: fixed filename in a live directory; flags do not allow directory removal.
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(error)
    }
}
fn rename(directory: &File, from: &CStr, to: &CStr) -> io::Result<()> {
    // SAFETY: fixed filenames, both anchored to the same live private directory.
    if unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
        )
    } == 0
    {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(error)
    }
}
