//! Persistent hash cache, keyed by path and invalidated by size + mtime.
//!
//! Stored in the OS cache directory (`~/.cache/find-duplicates/hashes.bin` on
//! Linux, `%LOCALAPPDATA%` on Windows, `~/Library/Caches` on macOS) using a
//! small custom binary format.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"FDHC";
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CacheEntry {
    pub size: u64,
    pub mtime: u64,
    pub quick: u64,
    pub full: Option<u64>,
}

#[derive(Default)]
pub struct HashCache {
    entries: HashMap<PathBuf, CacheEntry>,
}

impl HashCache {
    pub fn default_path() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("find-duplicates").join("hashes.bin"))
    }

    /// Loads the cache, returning an empty one if the file is missing or corrupt.
    pub fn load(path: &Path) -> Self {
        File::open(path)
            .and_then(|f| Self::read_from(BufReader::new(f)))
            .unwrap_or_default()
    }

    pub fn read_from(mut r: impl Read) -> io::Result<Self> {
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC || read_u32(&mut r)? != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad cache header",
            ));
        }
        let count = read_u64(&mut r)? as usize;
        let mut entries = HashMap::with_capacity(count);
        for _ in 0..count {
            let len = read_u32(&mut r)? as usize;
            let mut bytes = vec![0u8; len];
            r.read_exact(&mut bytes)?;
            let size = read_u64(&mut r)?;
            let mtime = read_u64(&mut r)?;
            let quick = read_u64(&mut r)?;
            let mut flag = [0u8; 1];
            r.read_exact(&mut flag)?;
            let full = read_u64(&mut r)?;
            if let Some(path) = path_from_bytes(bytes) {
                let full = (flag[0] == 1).then_some(full);
                entries.insert(
                    path,
                    CacheEntry {
                        size,
                        mtime,
                        quick,
                        full,
                    },
                );
            }
        }
        Ok(Self { entries })
    }

    /// Writes atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        self.write_to(BufWriter::new(File::create(&tmp)?))?;
        fs::rename(tmp, path)
    }

    pub fn write_to(&self, mut w: impl Write) -> io::Result<()> {
        {
            w.write_all(MAGIC)?;
            w.write_all(&VERSION.to_le_bytes())?;
            let encoded: Vec<_> = self
                .entries
                .iter()
                .filter_map(|(p, e)| path_to_bytes(p).map(|b| (b, e)))
                .collect();
            w.write_all(&(encoded.len() as u64).to_le_bytes())?;
            for (bytes, e) in encoded {
                w.write_all(&(bytes.len() as u32).to_le_bytes())?;
                w.write_all(bytes)?;
                w.write_all(&e.size.to_le_bytes())?;
                w.write_all(&e.mtime.to_le_bytes())?;
                w.write_all(&e.quick.to_le_bytes())?;
                w.write_all(&[e.full.is_some() as u8])?;
                w.write_all(&e.full.unwrap_or(0).to_le_bytes())?;
            }
            w.flush()?;
        }
        Ok(())
    }

    /// Returns the entry only if it is still valid for the given size/mtime.
    pub fn get(&self, path: &Path, size: u64, mtime: u64) -> Option<CacheEntry> {
        self.entries
            .get(path)
            .filter(|e| e.size == size && e.mtime == mtime)
            .copied()
    }

    pub fn insert(&mut self, path: PathBuf, entry: CacheEntry) {
        self.entries.insert(path, entry);
    }

    /// Drops entries under `root` whose file was not seen in the latest scan.
    pub fn prune(&mut self, root: &Path, seen: &HashSet<&Path>) {
        self.entries
            .retain(|p, _| !p.starts_with(root) || seen.contains(p.as_path()));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

#[cfg(unix)]
fn path_to_bytes(p: &Path) -> Option<&[u8]> {
    use std::os::unix::ffi::OsStrExt;
    Some(p.as_os_str().as_bytes())
}

#[cfg(unix)]
fn path_from_bytes(b: Vec<u8>) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(b)))
}

// Non-UTF-8 paths are simply not cached on other platforms.
#[cfg(not(unix))]
fn path_to_bytes(p: &Path) -> Option<&[u8]> {
    p.to_str().map(str::as_bytes)
}

#[cfg(not(unix))]
fn path_from_bytes(b: Vec<u8>) -> Option<PathBuf> {
    String::from_utf8(b).ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: CacheEntry = CacheEntry {
        size: 10,
        mtime: 5,
        quick: 1,
        full: Some(2),
    };

    fn roundtrip(cache: &HashCache) -> HashCache {
        let mut buf = Vec::new();
        cache.write_to(&mut buf).unwrap();
        HashCache::read_from(buf.as_slice()).unwrap()
    }

    #[test]
    fn roundtrip_and_invalidation() {
        let mut cache = HashCache::default();
        cache.insert(PathBuf::from("/a/b"), E);
        cache.insert(PathBuf::from("/a/c"), CacheEntry { full: None, ..E });

        let loaded = roundtrip(&cache);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(Path::new("/a/b"), 10, 5), Some(E));
        assert_eq!(loaded.get(Path::new("/a/c"), 10, 5).unwrap().full, None);
        // A changed mtime or size invalidates the entry.
        assert_eq!(loaded.get(Path::new("/a/b"), 10, 6), None);
        assert_eq!(loaded.get(Path::new("/a/b"), 11, 5), None);
    }

    #[test]
    fn non_ascii_paths_survive() {
        let mut cache = HashCache::default();
        cache.insert(PathBuf::from("/fotos/año 2015/canción ñ.mp3"), E);
        let loaded = roundtrip(&cache);
        assert_eq!(
            loaded.get(Path::new("/fotos/año 2015/canción ñ.mp3"), 10, 5),
            Some(E)
        );
    }

    #[test]
    fn rejects_corrupt_data() {
        assert!(HashCache::read_from(&b"NOPE\x01\0\0\0"[..]).is_err());
        assert!(HashCache::read_from(&b""[..]).is_err());

        let mut cache = HashCache::default();
        cache.insert(PathBuf::from("/a/b"), E);
        let mut buf = Vec::new();
        cache.write_to(&mut buf).unwrap();
        buf.truncate(buf.len() - 3);
        assert!(HashCache::read_from(buf.as_slice()).is_err());
    }

    #[test]
    fn load_missing_file_is_empty() {
        assert!(HashCache::load(Path::new("/nonexistent/find-duplicates/hashes.bin")).is_empty());
    }

    #[test]
    fn prune_only_touches_scanned_root() {
        let mut cache = HashCache::default();
        for p in ["/disk/kept", "/disk/deleted", "/other/untouched"] {
            cache.insert(PathBuf::from(p), E);
        }
        let seen: HashSet<&Path> = [Path::new("/disk/kept")].into_iter().collect();
        cache.prune(Path::new("/disk"), &seen);
        assert!(cache.get(Path::new("/disk/kept"), 10, 5).is_some());
        assert!(cache.get(Path::new("/disk/deleted"), 10, 5).is_none());
        assert!(cache.get(Path::new("/other/untouched"), 10, 5).is_some());
    }
}
