//! Walks a directory and assigns every file a content id, so that files with
//! the same id have the same content.
//!
//! Pipeline: group by size → xxh3 of head+tail (quick) → xxh3 of the whole
//! file for the remaining collisions (skipped in quick-scan mode). Hashes are
//! reused from [`HashCache`] when size and mtime are unchanged.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use walkdir::WalkDir;
use xxhash_rust::xxh3::{xxh3_64, Xxh3};

use crate::cache::{CacheEntry, HashCache};

/// Files up to this size are hashed whole in the quick pass.
const QUICK_WHOLE_LIMIT: u64 = 8192;
const QUICK_CHUNK: usize = 4096;

pub struct ScanOptions {
    pub ignore: HashSet<String>,
    pub min_size: u64,
    pub quick_scan: bool,
    pub cache_path: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Stage {
    Listing = 0,
    QuickHash = 1,
    FullHash = 2,
    Analyzing = 3,
}

impl Stage {
    pub const ALL: [Stage; 4] = [
        Stage::Listing,
        Stage::QuickHash,
        Stage::FullHash,
        Stage::Analyzing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Listing => "Listing files",
            Stage::QuickHash => "Quick hashing",
            Stage::FullHash => "Verifying full content",
            Stage::Analyzing => "Finding similar folders",
        }
    }
}

/// Shared between the scan thread and the UI.
#[derive(Default)]
pub struct Progress {
    stage: AtomicU8,
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn stage(&self) -> Stage {
        match self.stage.load(Ordering::Relaxed) {
            0 => Stage::Listing,
            1 => Stage::QuickHash,
            2 => Stage::FullHash,
            _ => Stage::Analyzing,
        }
    }

    pub fn set_stage(&self, stage: Stage, total: u64) {
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.stage.store(stage as u8, Ordering::Relaxed);
    }

    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| self.done.load(Ordering::Relaxed) as f32 / total as f32)
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: u64,
    pub content: u32,
}

pub struct ScanResult {
    pub root: PathBuf,
    /// Sorted by path, so every directory's descendants are a contiguous range.
    pub files: Vec<FileEntry>,
    /// Number of files sharing each content id.
    pub content_count: Vec<u32>,
    /// Content ids that appear more than once, as indices into `files`.
    pub groups: Vec<Vec<usize>>,
    /// Hashes served from the cache during this scan.
    pub cache_hits: usize,
}

impl ScanResult {
    pub fn is_duplicate(&self, idx: usize) -> bool {
        self.content_count[self.files[idx].content as usize] > 1
    }

    /// Index range in `files` of everything under `dir`.
    pub fn range_of(&self, dir: &Path) -> std::ops::Range<usize> {
        let start = self.files.partition_point(|f| f.path.as_path() < dir);
        let len = self.files[start..].partition_point(|f| f.path.starts_with(dir));
        start..start + len
    }

    pub fn wasted_bytes(&self) -> u64 {
        self.groups
            .iter()
            .map(|g| self.files[g[0]].size * (g.len() as u64 - 1))
            .sum()
    }
}

pub fn list_files(
    root: &Path,
    ignore: &HashSet<String>,
    min_size: u64,
    progress: &Progress,
) -> Vec<FileEntry> {
    let mut files = Vec::new();
    let walker = WalkDir::new(root).into_iter().filter_entry(|e| {
        e.file_name()
            .to_str()
            .is_none_or(|name| !ignore.contains(name))
    });
    for entry in walker.filter_map(|e| e.ok()) {
        if progress.cancelled() {
            break;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() < min_size {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        files.push(FileEntry {
            path: entry.into_path(),
            size: meta.len(),
            mtime,
            content: u32::MAX,
        });
        progress.done.fetch_add(1, Ordering::Relaxed);
    }
    files
}

fn read_up_to(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match file.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// Whole file for small files, first + last 4 KiB otherwise.
pub fn quick_hash(path: &Path, size: u64) -> io::Result<u64> {
    let mut file = File::open(path)?;
    if size <= QUICK_WHOLE_LIMIT {
        let mut buf = vec![0u8; size as usize];
        let n = read_up_to(&mut file, &mut buf)?;
        return Ok(xxh3_64(&buf[..n]));
    }
    let mut buf = [0u8; QUICK_CHUNK * 2];
    let head = read_up_to(&mut file, &mut buf[..QUICK_CHUNK])?;
    file.seek(SeekFrom::End(-(QUICK_CHUNK as i64)))?;
    let tail = read_up_to(&mut file, &mut buf[head..])?;
    Ok(xxh3_64(&buf[..head + tail]))
}

pub fn full_hash(path: &Path, progress: Option<&Progress>) -> io::Result<u64> {
    let mut file = File::open(path)?;
    let mut hasher = Xxh3::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        if let Some(p) = progress {
            p.done.fetch_add(n as u64, Ordering::Relaxed);
            if p.cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
        }
    }
    Ok(hasher.digest())
}

fn group_indices<K: std::hash::Hash + Eq>(
    items: impl Iterator<Item = (K, usize)>,
) -> impl Iterator<Item = Vec<usize>> {
    let mut map: HashMap<K, Vec<usize>> = HashMap::new();
    for (k, i) in items {
        map.entry(k).or_default().push(i);
    }
    map.into_values().filter(|v| v.len() > 1)
}

pub fn scan(root: &Path, opts: &ScanOptions, progress: &Progress) -> Option<ScanResult> {
    progress.set_stage(Stage::Listing, 0);
    let mut files = list_files(root, &opts.ignore, opts.min_size, progress);
    if progress.cancelled() {
        return None;
    }
    files.sort_unstable_by(|a, b| a.path.cmp(&b.path));

    let mut cache = opts
        .cache_path
        .as_deref()
        .map(HashCache::load)
        .unwrap_or_default();
    let mut cache_hits = 0usize;

    // Quick pass on files whose size is shared with some other file.
    let candidates: Vec<usize> = group_indices(files.iter().enumerate().map(|(i, f)| (f.size, i)))
        .flatten()
        .collect();
    progress.set_stage(Stage::QuickHash, candidates.len() as u64);

    let quick: Vec<(usize, Option<CacheEntry>, bool)> = candidates
        .par_iter()
        .map(|&i| {
            let f = &files[i];
            let res = if let Some(e) = cache.get(&f.path, f.size, f.mtime) {
                (i, Some(e), true)
            } else if progress.cancelled() {
                (i, None, false)
            } else {
                let entry = quick_hash(&f.path, f.size).ok().map(|q| CacheEntry {
                    size: f.size,
                    mtime: f.mtime,
                    quick: q,
                    full: (f.size <= QUICK_WHOLE_LIMIT).then_some(q),
                });
                (i, entry, false)
            };
            progress.done.fetch_add(1, Ordering::Relaxed);
            res
        })
        .collect();
    if progress.cancelled() {
        return None;
    }

    let mut entries: HashMap<usize, CacheEntry> = HashMap::with_capacity(quick.len());
    for (i, entry, hit) in quick {
        if let Some(e) = entry {
            cache_hits += hit as usize;
            entries.insert(i, e);
        }
    }

    // Full pass on quick-hash collisions.
    let quick_groups: Vec<Vec<usize>> =
        group_indices(entries.iter().map(|(&i, e)| ((e.size, e.quick), i))).collect();
    if !opts.quick_scan {
        let need_full: Vec<usize> = quick_groups
            .iter()
            .flatten()
            .copied()
            .filter(|i| entries[i].full.is_none())
            .collect();
        let total_bytes = need_full.iter().map(|&i| files[i].size).sum();
        progress.set_stage(Stage::FullHash, total_bytes);
        let fulls: Vec<(usize, Option<u64>)> = need_full
            .par_iter()
            .map(|&i| (i, full_hash(&files[i].path, Some(progress)).ok()))
            .collect();
        if progress.cancelled() {
            return None;
        }
        for (i, full) in fulls {
            match full {
                Some(h) => entries.get_mut(&i).unwrap().full = Some(h),
                None => {
                    entries.remove(&i);
                }
            }
        }
    }

    // Assign content ids. Without a full hash (quick scan) the quick hash is the key.
    let mut ids: HashMap<(u64, u64), u32> = HashMap::new();
    let mut content_count: Vec<u32> = Vec::new();
    for (i, f) in files.iter_mut().enumerate() {
        let key = entries.get(&i).map(|e| {
            let h = if opts.quick_scan {
                e.quick
            } else {
                e.full.unwrap_or(e.quick)
            };
            (f.size, h)
        });
        let id = match key {
            Some(k) => *ids.entry(k).or_insert_with(|| {
                content_count.push(0);
                content_count.len() as u32 - 1
            }),
            None => {
                content_count.push(0);
                content_count.len() as u32 - 1
            }
        };
        content_count[id as usize] += 1;
        f.content = id;
    }

    let mut by_content: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, f) in files.iter().enumerate() {
        if content_count[f.content as usize] > 1 {
            by_content.entry(f.content).or_default().push(i);
        }
    }
    let groups = by_content.into_values().collect();

    if let Some(path) = &opts.cache_path {
        for (i, e) in entries {
            cache.insert(files[i].path.clone(), e);
        }
        let seen: HashSet<&Path> = files.iter().map(|f| f.path.as_path()).collect();
        cache.prune(root, &seen);
        if let Err(err) = cache.save(path) {
            eprintln!("Warning: could not save hash cache ({err}): {path:?}");
        }
    }

    Some(ScanResult {
        root: root.to_path_buf(),
        files,
        content_count,
        groups,
        cache_hits,
    })
}
