//! File system access used by the scanner, behind a trait so tests can run
//! against an in-memory disk instead of writing real files.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use walkdir::WalkDir;

pub struct FileMeta {
    pub path: PathBuf,
    pub size: u64,
    /// Nanoseconds since the Unix epoch, 0 if unknown.
    pub mtime: u64,
}

pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

pub trait FileSystem: Sync {
    /// Calls `visit` for every regular file under `root`, without following
    /// symlinks and skipping entries whose name is in `ignore`. Stops early
    /// when `visit` returns `false`.
    fn walk(&self, root: &Path, ignore: &HashSet<String>, visit: &mut dyn FnMut(FileMeta) -> bool);

    fn open(&self, path: &Path) -> io::Result<Box<dyn ReadSeek + '_>>;
}

pub struct RealFs;

impl FileSystem for RealFs {
    fn walk(&self, root: &Path, ignore: &HashSet<String>, visit: &mut dyn FnMut(FileMeta) -> bool) {
        let walker = WalkDir::new(root).into_iter().filter_entry(|e| {
            e.file_name()
                .to_str()
                .is_none_or(|name| !ignore.contains(name))
        });
        for entry in walker.filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let file = FileMeta {
                path: entry.into_path(),
                size: meta.len(),
                mtime,
            };
            if !visit(file) {
                break;
            }
        }
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn ReadSeek + '_>> {
        Ok(Box::new(File::open(path)?))
    }
}

#[cfg(test)]
pub mod mem {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MemFile {
        data: Vec<u8>,
        mtime: u64,
        unreadable: bool,
    }

    /// In-memory disk. Paths given to the builder are relative to [`MemFs::ROOT`].
    #[derive(Default)]
    pub struct MemFs {
        files: BTreeMap<PathBuf, MemFile>,
        opens: AtomicUsize,
    }

    impl MemFs {
        pub const ROOT: &str = "/disk";

        pub fn new() -> Self {
            Self::default()
        }

        pub fn path(rel: &str) -> PathBuf {
            Path::new(Self::ROOT).join(rel)
        }

        pub fn root() -> PathBuf {
            PathBuf::from(Self::ROOT)
        }

        pub fn file(mut self, rel: &str, data: impl Into<Vec<u8>>) -> Self {
            self.write(rel, data);
            self
        }

        /// Adds a file that is listed but fails to open (e.g. no permission).
        pub fn unreadable(mut self, rel: &str, data: impl Into<Vec<u8>>) -> Self {
            self.write(rel, data);
            self.files.get_mut(&Self::path(rel)).unwrap().unreadable = true;
            self
        }

        /// Creates or overwrites a file, bumping its mtime.
        pub fn write(&mut self, rel: &str, data: impl Into<Vec<u8>>) {
            let path = Self::path(rel);
            let mtime = self.files.get(&path).map_or(1, |f| f.mtime + 1);
            self.files.insert(
                path,
                MemFile {
                    data: data.into(),
                    mtime,
                    unreadable: false,
                },
            );
        }

        /// Number of times any file has been opened.
        pub fn opens(&self) -> usize {
            self.opens.load(Ordering::Relaxed)
        }
    }

    impl FileSystem for MemFs {
        fn walk(
            &self,
            root: &Path,
            ignore: &HashSet<String>,
            visit: &mut dyn FnMut(FileMeta) -> bool,
        ) {
            for (path, file) in &self.files {
                let Ok(rel) = path.strip_prefix(root) else {
                    continue;
                };
                let ignored = rel
                    .components()
                    .any(|c| c.as_os_str().to_str().is_some_and(|n| ignore.contains(n)));
                if ignored {
                    continue;
                }
                let meta = FileMeta {
                    path: path.clone(),
                    size: file.data.len() as u64,
                    mtime: file.mtime,
                };
                if !visit(meta) {
                    break;
                }
            }
        }

        fn open(&self, path: &Path) -> io::Result<Box<dyn ReadSeek + '_>> {
            self.opens.fetch_add(1, Ordering::Relaxed);
            match self.files.get(path) {
                Some(f) if f.unreadable => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
                Some(f) => Ok(Box::new(Cursor::new(f.data.as_slice()))),
                None => Err(io::Error::from(io::ErrorKind::NotFound)),
            }
        }
    }
}
