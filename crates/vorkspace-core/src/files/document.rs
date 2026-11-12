//! Bounded file reads and atomic editor saves, extracted from Tauri's fs service.
use super::*;
use std::io::Read;

pub const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_LINE_BYTES: usize = 16 * 1024;
const MAX_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalRevision {
    device: u64,
    inode: u64,
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

/// A cheap check for a local preview. The save path still compares exact bytes.
pub fn local_revision(path: &Path) -> Option<LocalRevision> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(expand_home(path)).ok()?;
    meta.is_file().then(|| LocalRevision {
        device: meta.dev(),
        inode: meta.ino(),
        len: meta.len(),
        modified: (meta.mtime(), meta.mtime_nsec()),
        changed: (meta.ctime(), meta.ctime_nsec()),
    })
}

#[derive(Clone, Debug)]
pub struct TextDocument {
    pub text: String,
    pub crlf: bool,
    pub cr: bool,
    pub bom: bool,
    /// Original bytes are retained to detect an external edit before saving.
    pub disk: Vec<u8>,
}
#[derive(Debug)]
pub enum Document {
    Text(TextDocument),
    Image {
        bytes: Vec<u8>,
        format: &'static str,
        width: u32,
        height: u32,
        original_size: usize,
    },
    Video {
        size: u64,
        modified: Option<std::time::SystemTime>,
    },
    Unsupported(String),
}

fn private_temporary(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
    options.open(path).map_err(|error| error.to_string())
}

fn open_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

/// Resolve an editor link against its workspace without touching disk.
pub fn resolve_path(path: &Path, cwd: &Path) -> PathBuf {
    let path = expand_home(path);
    let path = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    let mut clean = PathBuf::new();
    for component in path.components() {
        if component == std::path::Component::ParentDir {
            clean.pop();
        } else if component != std::path::Component::CurDir {
            clean.push(component);
        }
    }
    clean
}

pub fn read_document(path: &Path) -> Result<Document, String> {
    let path = expand_home(path);
    let file = open_file(&path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Not a regular file".into());
    }
    if is_video_path(&path) {
        return Ok(Document::Video {
            size: meta.len(),
            modified: meta.modified().ok(),
        });
    }
    if meta.len() > MAX_PREVIEW_BYTES {
        return Ok(Document::Unsupported(
            "File is too large to preview (maximum 32 MB).".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_PREVIEW_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_PREVIEW_BYTES {
        return Ok(Document::Unsupported(
            "File is too large to preview (maximum 32 MB).".into(),
        ));
    }
    document_from_bytes(&path, bytes, MAX_TEXT_BYTES)
}

fn document_from_bytes(
    path: &Path,
    bytes: Vec<u8>,
    max_text_bytes: usize,
) -> Result<Document, String> {
    let format = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else if bytes.starts_with(b"\x00\x00\x01\x00") {
        Some("ico")
    } else if bytes.get(4..8) == Some(b"ftyp")
        && matches!(bytes.get(8..12), Some(b"avif" | b"avis"))
    {
        Some("avif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    };
    if let Some(format) = format {
        let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok())
            .unwrap_or((0, 0));
        if (width == 0 || height == 0) && format != "avif" {
            return Ok(Document::Unsupported("Not a readable image.".into()));
        }
        return Ok(Document::Image {
            original_size: bytes.len(),
            bytes,
            format,
            width,
            height,
        });
    }
    if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico"
            )
        })
    {
        return Ok(Document::Unsupported("Not a readable image.".into()));
    }
    if bytes.len() > max_text_bytes {
        return Ok(Document::Unsupported(format!(
            "File is too large to edit (maximum {} MB).",
            max_text_bytes / 1024 / 1024
        )));
    }
    if bytes.contains(&0) {
        return Ok(Document::Unsupported(
            "Binary files cannot be edited.".into(),
        ));
    }
    let bom = bytes.starts_with(b"\xef\xbb\xbf");
    let Ok(raw) = std::str::from_utf8(if bom { &bytes[3..] } else { &bytes }) else {
        return Ok(Document::Unsupported("This file is not UTF-8 text.".into()));
    };
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    if text.lines().any(|line| line.len() > MAX_LINE_BYTES) {
        return Ok(Document::Unsupported(
            "This file contains lines too long to edit (maximum 16 KB per line).".into(),
        ));
    }
    let crlf = raw.contains("\r\n");
    Ok(Document::Text(TextDocument {
        text,
        crlf,
        cr: !crlf && raw.contains('\r'),
        bom,
        disk: bytes,
    }))
}

pub(crate) fn is_video_path(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "mp4"
                    | "m4v"
                    | "mov"
                    | "webm"
                    | "ogv"
                    | "avi"
                    | "mkv"
                    | "mpg"
                    | "mpeg"
                    | "3gp"
                    | "3g2"
            )
        })
}

impl TextDocument {
    pub(crate) fn encode(&self, text: &str) -> Vec<u8> {
        let mut out = if self.bom {
            vec![0xef, 0xbb, 0xbf]
        } else {
            Vec::new()
        };
        if self.crlf {
            out.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
        } else if self.cr {
            out.extend_from_slice(text.replace('\n', "\r").as_bytes());
        } else {
            out.extend_from_slice(text.as_bytes());
        }
        out
    }
}

/// Preserve permissions and symlinks, and refuse to overwrite a changed file.
pub fn save_document(path: &Path, document: &TextDocument, text: &str) -> Result<Vec<u8>, String> {
    if text.len() > MAX_TEXT_BYTES || text.split('\n').any(|line| line.len() > MAX_LINE_BYTES) {
        return Err("Text exceeds the editor size limit.".into());
    }

    let destination = std::fs::canonicalize(expand_home(path)).map_err(|e| e.to_string())?;
    if !matches_disk(&destination, &document.disk)? {
        return Err("The file changed on disk. Reload it before saving your edits.".into());
    }
    let parent = destination.parent().ok_or("File has no parent directory")?;
    let name = destination
        .file_name()
        .ok_or("Invalid file name")?
        .to_string_lossy();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        ".{name}.vorkspace-{}-{stamp}.tmp",
        std::process::id()
    ));
    let bytes = document.encode(text);
    let mut file = private_temporary(&temporary)?;
    let result = (|| {
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        let permissions = std::fs::metadata(&destination)
            .map_err(|e| e.to_string())?
            .permissions();
        std::fs::set_permissions(&temporary, permissions).map_err(|e| e.to_string())?;
        // Check again after writing, so an edit during the save is also caught.
        if !matches_disk(&destination, &document.disk)? {
            return Err("The file changed on disk. Reload it before saving your edits.".into());
        }
        drop(file);
        std::fs::rename(&temporary, &destination).map_err(|e| e.to_string())?;
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(bytes)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn matches_disk(path: &Path, expected: &[u8]) -> Result<bool, String> {
    let file = open_file(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() != expected.len() as u64 {
        return Ok(false);
    }
    let mut bytes = Vec::with_capacity(expected.len());
    file.take(expected.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_preview_revision_tracks_same_size_atomic_replacements_through_links() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.txt");
        let link = directory.path().join("link.txt");
        let replacement = directory.path().join("replacement.txt");
        std::fs::write(&target, "old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let before = local_revision(&link).unwrap();
        assert_eq!(before, local_revision(&link).unwrap());
        std::fs::write(&replacement, "new").unwrap();
        std::fs::rename(&replacement, &target).unwrap();
        assert_ne!(before, local_revision(&link).unwrap());
        std::fs::remove_file(&target).unwrap();
        assert!(local_revision(&link).is_none());
    }

    #[test]
    fn temporary_document_bytes_are_private_before_publication_and_never_replace_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private.tmp");
        let mut file = private_temporary(&path).unwrap();
        file.write_all(b"unfinished private document").unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(private_temporary(&path).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"unfinished private document"
        );
    }
    #[test]
    fn atomic_saves_preserve_crlf_bom_permissions_and_reject_external_changes() {
        let root = std::env::temp_dir().join(format!("vorkspace-document-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("source.rs");
        std::fs::write(&path, b"\xef\xbb\xbffn main() {}\r\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let Document::Text(mut doc) = read_document(&path).unwrap() else {
            panic!()
        };
        assert_eq!(doc.text, "fn main() {}\n");
        doc.disk = save_document(&path, &doc, "// 🙂\n").unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            "\u{feff}// 🙂\r\n".as_bytes()
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        std::fs::write(&path, "external").unwrap();
        assert!(save_document(&path, &doc, "draft")
            .unwrap_err()
            .contains("changed on disk"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn editor_saves_keep_each_line_ending_convention() {
        let path =
            std::env::temp_dir().join(format!("vorkspace-line-endings-{}.txt", std::process::id()));
        for eol in ["\n", "\r\n", "\r"] {
            std::fs::write(&path, format!("first{eol}second{eol}")).unwrap();
            let Document::Text(doc) = read_document(&path).unwrap() else {
                panic!("text document")
            };
            assert_eq!(doc.text, "first\nsecond\n");
            save_document(&path, &doc, "🙂\nchanged\n").unwrap();
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                format!("🙂{eol}changed{eol}")
            );
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn binary_invalid_utf8_and_long_lines_are_explicit() {
        let path =
            std::env::temp_dir().join(format!("vorkspace-document-types-{}", std::process::id()));
        for bytes in [
            vec![0],
            vec![0xff],
            vec![b'x'; MAX_LINE_BYTES + 1],
            vec![b'x'; MAX_TEXT_BYTES + 1],
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(matches!(
                read_document(&path).unwrap(),
                Document::Unsupported(_)
            ));
        }
        std::fs::write(&path, b"").unwrap();
        assert!(matches!(read_document(&path).unwrap(), Document::Text(_)));
        std::fs::remove_file(path).unwrap();
    }
}
