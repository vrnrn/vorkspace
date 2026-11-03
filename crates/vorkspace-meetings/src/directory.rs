//! Directory-relative storage operations. Open each ancestor without following links.
use rustix::{
    fd::OwnedFd,
    fs::{self, AtFlags, Mode, OFlags},
    io::Errno,
};
use std::{
    ffi::OsStr,
    fs::File,
    io,
    path::{Component, Path, PathBuf},
};

const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const PRIVATE_DIRECTORY: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::XUSR);
const PRIVATE_FILE: Mode = Mode::RUSR.union(Mode::WUSR);

pub(crate) struct Directory(OwnedFd);

impl Directory {
    pub(crate) fn open(path: &Path, create: bool) -> io::Result<Self> {
        let path = system_alias(path);
        let mut dir = Self(fs::open("/", DIRECTORY, Mode::empty())?);
        if !path.is_absolute() {
            return Err(Errno::INVAL.into());
        }
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => {
                    dir = match dir.child(name) {
                        Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                            match fs::mkdirat(&dir.0, name, PRIVATE_DIRECTORY) {
                                Ok(()) | Err(Errno::EXIST) => dir.child(name)?,
                                Err(error) => return Err(error.into()),
                            }
                        }
                        result => result?,
                    };
                }
                _ => return Err(Errno::INVAL.into()),
            }
        }
        Ok(dir)
    }

    fn child(&self, name: &OsStr) -> io::Result<Self> {
        Ok(Self(fs::openat(&self.0, name, DIRECTORY, Mode::empty())?))
    }

    pub(crate) fn create_child(&self, name: &OsStr) -> io::Result<Self> {
        fs::mkdirat(&self.0, name, PRIVATE_DIRECTORY)?;
        self.child(name)
    }

    pub(crate) fn file(&self, name: &OsStr, flags: OFlags) -> io::Result<File> {
        Ok(fs::openat(
            &self.0,
            name,
            flags | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            PRIVATE_FILE,
        )?
        .into())
    }

    pub(crate) fn entries(&self) -> io::Result<fs::Dir> {
        Ok(fs::Dir::read_from(&self.0)?)
    }

    pub(crate) fn publish(&self, temporary: &OsStr, name: &OsStr) -> io::Result<()> {
        fs::renameat(&self.0, temporary, &self.0, name)?;
        fs::fsync(&self.0)?;
        Ok(())
    }

    pub(crate) fn remove(&self, name: &OsStr) {
        let _ = fs::unlinkat(&self.0, name, AtFlags::empty());
    }
}

fn system_alias(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    for alias in ["/var", "/tmp"] {
        if let Ok(rest) = path.strip_prefix(alias) {
            return Path::new("/private").join(&alias[1..]).join(rest);
        }
    }
    path.into()
}
