//! Folder-level analysis: finds pairs of folders that are copies (identical,
//! one contained in the other, or similar) and compares them file by file.
//!
//! Candidate pairs come from "copy root" voting: for every pair of duplicate
//! files, the longest common path suffix tells where the two copies are
//! rooted, e.g. `Backup/PC/Docs/a/x.pdf` and `Old/Docs/a/x.pdf` vote for
//! (`Backup/PC/Docs`, `Old/Docs`). Every candidate is then measured exactly.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::scan::ScanResult;

/// Groups larger than this only contribute pairs among their first members,
/// to avoid quadratic blowups on very common files.
const MAX_GROUP_FOR_VOTING: usize = 48;
const MAX_CANDIDATES: usize = 5000;
/// Pairs where neither side has at least this fraction of its files in the
/// other are coincidences rather than copies.
const MIN_RATIO: f32 = 0.1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Relation {
    Identical,
    /// Every file of A also exists somewhere in B.
    AInB,
    BInA,
    Similar,
}

#[derive(Clone, Debug)]
pub struct FolderSide {
    pub path: PathBuf,
    pub range: Range<usize>,
    pub bytes: u64,
    /// Files whose content also exists on the other side.
    pub shared_files: usize,
    pub shared_bytes: u64,
}

impl FolderSide {
    pub fn files(&self) -> usize {
        self.range.len()
    }

    /// Fraction of this folder's files found in the other folder.
    pub fn ratio(&self) -> f32 {
        if self.range.is_empty() {
            0.0
        } else {
            self.shared_files as f32 / self.range.len() as f32
        }
    }

    pub fn unique_files(&self) -> usize {
        self.files() - self.shared_files
    }
}

#[derive(Clone, Debug)]
pub struct FolderPair {
    pub a: FolderSide,
    pub b: FolderSide,
    pub votes: u32,
}

impl FolderPair {
    pub fn relation(&self) -> Relation {
        let a_full = self.a.shared_files == self.a.files();
        let b_full = self.b.shared_files == self.b.files();
        match (a_full, b_full) {
            (true, true) if self.a.files() == self.b.files() => Relation::Identical,
            (true, _) => Relation::AInB,
            (_, true) => Relation::BInA,
            _ => Relation::Similar,
        }
    }

    /// Overall similarity (Jaccard-like, by file count).
    pub fn similarity(&self) -> f32 {
        let shared = self.a.shared_files + self.b.shared_files;
        let total = self.a.files() + self.b.files();
        if total == 0 {
            0.0
        } else {
            shared as f32 / total as f32
        }
    }

    /// Bytes freed by deleting the side that is most covered by the other,
    /// counting only files that have a copy on the other side.
    pub fn reclaimable(&self) -> u64 {
        if self.a.ratio() >= self.b.ratio() {
            self.a.shared_bytes
        } else {
            self.b.shared_bytes
        }
    }

    /// Ranking that favours real copies over folders sharing a few big files.
    pub fn relevance(&self) -> f64 {
        self.reclaimable() as f64 * self.a.ratio().max(self.b.ratio()) as f64
    }

    fn covers(&self, other: &FolderPair) -> bool {
        let inside = |p: &FolderPair, q: &FolderPair| {
            q.a.path.starts_with(&p.a.path) && q.b.path.starts_with(&p.b.path)
        };
        let swapped =
            other.a.path.starts_with(&self.b.path) && other.b.path.starts_with(&self.a.path);
        (inside(self, other) || swapped)
            && self.a.ratio().max(self.b.ratio()) >= other.a.ratio().max(other.b.ratio())
    }
}

fn common_suffix_len(a: &Path, b: &Path) -> usize {
    a.components()
        .rev()
        .zip(b.components().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

fn strip_components(p: &Path, n: usize) -> Option<&Path> {
    let mut p = p;
    for _ in 0..n {
        p = p.parent()?;
    }
    Some(p)
}

/// The two folders where a pair of identical files is rooted, or `None` if
/// they share a folder or one root would contain the other.
fn copy_roots<'a>(root: &Path, a: &'a Path, b: &'a Path) -> Option<(&'a Path, &'a Path)> {
    let k = common_suffix_len(a, b);
    // With suffix `Docs/a/x.pdf` (k=3) the roots are the `Docs` folders.
    for strip in (1..=k.saturating_sub(1).max(1)).rev() {
        let ra = strip_components(a, strip)?;
        let rb = strip_components(b, strip)?;
        if !ra.starts_with(root) || !rb.starts_with(root) {
            continue;
        }
        if ra.starts_with(rb) || rb.starts_with(ra) {
            continue;
        }
        return Some((ra, rb));
    }
    None
}

fn vote(scan: &ScanResult) -> HashMap<(PathBuf, PathBuf), u32> {
    scan.groups
        .par_iter()
        .fold(
            HashMap::new,
            |mut votes: HashMap<(PathBuf, PathBuf), u32>, group| {
                let members = &group[..group.len().min(MAX_GROUP_FOR_VOTING)];
                let mut seen_here: HashSet<(&Path, &Path)> = HashSet::new();
                for (n, &i) in members.iter().enumerate() {
                    for &j in &members[n + 1..] {
                        let (pa, pb) = (&scan.files[i].path, &scan.files[j].path);
                        let Some((ra, rb)) = copy_roots(&scan.root, pa, pb) else {
                            continue;
                        };
                        let key = if ra < rb { (ra, rb) } else { (rb, ra) };
                        if seen_here.insert(key) {
                            *votes
                                .entry((key.0.to_path_buf(), key.1.to_path_buf()))
                                .or_default() += 1;
                        }
                    }
                }
                votes
            },
        )
        .reduce(HashMap::new, |mut a, b| {
            for (k, v) in b {
                *a.entry(k).or_default() += v;
            }
            a
        })
}

fn measure_side(
    scan: &ScanResult,
    path: &Path,
    range: Range<usize>,
    other: &HashSet<u32>,
) -> FolderSide {
    let mut side = FolderSide {
        path: path.to_path_buf(),
        range: range.clone(),
        bytes: 0,
        shared_files: 0,
        shared_bytes: 0,
    };
    for f in &scan.files[range] {
        side.bytes += f.size;
        if other.contains(&f.content) {
            side.shared_files += 1;
            side.shared_bytes += f.size;
        }
    }
    side
}

pub fn measure_pair(scan: &ScanResult, a: &Path, b: &Path, votes: u32) -> FolderPair {
    let ra = scan.range_of(a);
    let rb = scan.range_of(b);
    let set = |r: &Range<usize>| -> HashSet<u32> {
        scan.files[r.clone()].iter().map(|f| f.content).collect()
    };
    let (set_a, set_b) = (set(&ra), set(&rb));
    let mut pair = FolderPair {
        a: measure_side(scan, a, ra, &set_b),
        b: measure_side(scan, b, rb, &set_a),
        votes,
    };
    // A is the likely redundant copy: the side most covered by the other,
    // or on a tie the more deeply nested one (backups tend to be nested).
    let depth = |s: &FolderSide| s.path.components().count();
    if (pair.b.ratio(), depth(&pair.b)) > (pair.a.ratio(), depth(&pair.a)) {
        std::mem::swap(&mut pair.a, &mut pair.b);
    }
    pair
}

/// Folder pairs sorted by relevance, without pairs nested inside a
/// reported pair that is at least as similar.
pub fn find_folder_pairs(scan: &ScanResult) -> Vec<FolderPair> {
    let mut candidates: Vec<((PathBuf, PathBuf), u32)> = vote(scan).into_iter().collect();
    candidates.sort_unstable_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
    candidates.truncate(MAX_CANDIDATES);

    let mut pairs: Vec<FolderPair> = candidates
        .par_iter()
        .map(|((a, b), votes)| measure_pair(scan, a, b, *votes))
        .filter(|p| p.a.shared_files > 0 && p.b.shared_files > 0)
        .filter(|p| p.a.ratio().max(p.b.ratio()) >= MIN_RATIO)
        .collect();
    pairs.sort_unstable_by(|x, y| {
        y.relevance()
            .total_cmp(&x.relevance())
            .then_with(|| x.a.path.cmp(&y.a.path))
    });

    // Index kept pairs by their `a` path so nested lookups walk ancestors only.
    let mut kept: Vec<FolderPair> = Vec::new();
    let mut by_path: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for pair in pairs {
        let covered = [&pair.a.path, &pair.b.path].iter().any(|p| {
            p.ancestors()
                .filter_map(|anc| by_path.get(anc))
                .flatten()
                .any(|&k| kept[k].covers(&pair))
        });
        if !covered {
            let idx = kept.len();
            by_path.entry(pair.a.path.clone()).or_default().push(idx);
            by_path.entry(pair.b.path.clone()).or_default().push(idx);
            kept.push(pair);
        }
    }
    kept
}

// ---------------------------------------------------------------------------
// Side-by-side comparison

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileStatus {
    /// Same relative path, same content.
    Same,
    /// Same relative path, different content.
    Modified,
    /// Only in A by path, but the content exists elsewhere in B.
    MovedA,
    MovedB,
    /// Only in A and its content is nowhere in B: lost if A is deleted.
    UniqueA,
    UniqueB,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct StatusCounts {
    pub same: usize,
    pub modified: usize,
    pub moved_a: usize,
    pub moved_b: usize,
    pub unique_a: usize,
    pub unique_b: usize,
}

impl StatusCounts {
    fn add(&mut self, s: FileStatus) {
        match s {
            FileStatus::Same => self.same += 1,
            FileStatus::Modified => self.modified += 1,
            FileStatus::MovedA => self.moved_a += 1,
            FileStatus::MovedB => self.moved_b += 1,
            FileStatus::UniqueA => self.unique_a += 1,
            FileStatus::UniqueB => self.unique_b += 1,
        }
    }

    fn merge(&mut self, o: &StatusCounts) {
        self.same += o.same;
        self.modified += o.modified;
        self.moved_a += o.moved_a;
        self.moved_b += o.moved_b;
        self.unique_a += o.unique_a;
        self.unique_b += o.unique_b;
    }

    pub fn differences(&self) -> usize {
        self.modified + self.moved_a + self.moved_b + self.unique_a + self.unique_b
    }
}

#[derive(Clone, Debug)]
pub struct CompareNode {
    pub name: String,
    pub depth: usize,
    pub children: Vec<usize>,
    /// For files: index into `ScanResult::files` on each side.
    pub file_a: Option<usize>,
    pub file_b: Option<usize>,
    pub status: Option<FileStatus>,
    pub counts: StatusCounts,
}

impl CompareNode {
    pub fn is_dir(&self) -> bool {
        self.status.is_none()
    }
}

/// Merged tree of both folders by relative path. Node 0 is the root.
pub struct CompareTree {
    pub nodes: Vec<CompareNode>,
}

impl CompareTree {
    pub fn build(scan: &ScanResult, pair: &FolderPair) -> Self {
        let contents = |r: &Range<usize>| -> HashSet<u32> {
            scan.files[r.clone()].iter().map(|f| f.content).collect()
        };
        let (in_a, in_b) = (contents(&pair.a.range), contents(&pair.b.range));

        let mut by_rel: HashMap<&Path, (Option<usize>, Option<usize>)> = HashMap::new();
        for i in pair.a.range.clone() {
            let rel = scan.files[i]
                .path
                .strip_prefix(&pair.a.path)
                .unwrap_or(&scan.files[i].path);
            by_rel.entry(rel).or_default().0 = Some(i);
        }
        for i in pair.b.range.clone() {
            let rel = scan.files[i]
                .path
                .strip_prefix(&pair.b.path)
                .unwrap_or(&scan.files[i].path);
            by_rel.entry(rel).or_default().1 = Some(i);
        }
        let mut rels: Vec<_> = by_rel.into_iter().collect();
        rels.sort_unstable_by(|x, y| x.0.cmp(y.0));

        let mut tree = CompareTree {
            nodes: vec![CompareNode {
                name: String::new(),
                depth: 0,
                children: Vec::new(),
                file_a: None,
                file_b: None,
                status: None,
                counts: StatusCounts::default(),
            }],
        };
        let mut dir_index: HashMap<PathBuf, usize> = HashMap::new();
        for (rel, (fa, fb)) in rels {
            let status = match (fa, fb) {
                (Some(a), Some(b)) if scan.files[a].content == scan.files[b].content => {
                    FileStatus::Same
                }
                (Some(_), Some(_)) => FileStatus::Modified,
                (Some(a), None) if in_b.contains(&scan.files[a].content) => FileStatus::MovedA,
                (Some(_), None) => FileStatus::UniqueA,
                (None, Some(b)) if in_a.contains(&scan.files[b].content) => FileStatus::MovedB,
                (None, Some(_)) => FileStatus::UniqueB,
                (None, None) => unreachable!(),
            };
            let parent = match rel.parent().filter(|p| !p.as_os_str().is_empty()) {
                Some(dir) => tree.ensure_dir(dir, &mut dir_index),
                None => 0,
            };
            let idx = tree.nodes.len();
            tree.nodes.push(CompareNode {
                name: rel
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                depth: tree.nodes[parent].depth + 1,
                children: Vec::new(),
                file_a: fa,
                file_b: fb,
                status: Some(status),
                counts: StatusCounts::default(),
            });
            tree.nodes[idx].counts.add(status);
            tree.nodes[parent].children.push(idx);
        }
        tree.aggregate(0);
        tree.sort_children();
        tree
    }

    fn ensure_dir(&mut self, dir: &Path, index: &mut HashMap<PathBuf, usize>) -> usize {
        if let Some(&i) = index.get(dir) {
            return i;
        }
        let parent = match dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            Some(p) => self.ensure_dir(p, index),
            None => 0,
        };
        let idx = self.nodes.len();
        self.nodes.push(CompareNode {
            name: dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            depth: self.nodes[parent].depth + 1,
            children: Vec::new(),
            file_a: None,
            file_b: None,
            status: None,
            counts: StatusCounts::default(),
        });
        self.nodes[parent].children.push(idx);
        index.insert(dir.to_path_buf(), idx);
        idx
    }

    fn aggregate(&mut self, idx: usize) -> StatusCounts {
        if !self.nodes[idx].is_dir() {
            return self.nodes[idx].counts;
        }
        let mut total = StatusCounts::default();
        for c in self.nodes[idx].children.clone() {
            total.merge(&self.aggregate(c));
        }
        self.nodes[idx].counts = total;
        total
    }

    /// Directories first, then by name.
    fn sort_children(&mut self) {
        for i in 0..self.nodes.len() {
            let mut children = std::mem::take(&mut self.nodes[i].children);
            children.sort_by(|&x, &y| {
                let (nx, ny) = (&self.nodes[x], &self.nodes[y]);
                ny.is_dir()
                    .cmp(&nx.is_dir())
                    .then_with(|| nx.name.cmp(&ny.name))
            });
            self.nodes[i].children = children;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{scan, Progress, ScanOptions};
    use std::fs;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn scan_dir(root: &Path) -> ScanResult {
        let opts = ScanOptions {
            ignore: HashSet::new(),
            min_size: 0,
            quick_scan: false,
            cache_path: None,
        };
        scan(root, &opts, &Progress::default()).unwrap()
    }

    #[test]
    fn copy_roots_uses_common_suffix() {
        let root = Path::new("/d");
        let (a, b) = copy_roots(
            root,
            Path::new("/d/Backup/PC/Docs/a/x.pdf"),
            Path::new("/d/Old/Docs/a/x.pdf"),
        )
        .unwrap();
        assert_eq!(a, Path::new("/d/Backup/PC/Docs"));
        assert_eq!(b, Path::new("/d/Old/Docs"));

        let (a, b) = copy_roots(root, Path::new("/d/x/f.txt"), Path::new("/d/y/g.txt")).unwrap();
        assert_eq!((a, b), (Path::new("/d/x"), Path::new("/d/y")));

        // Same folder or nested folders are not copy roots.
        assert!(copy_roots(root, Path::new("/d/x/f"), Path::new("/d/x/g")).is_none());
        assert!(copy_roots(root, Path::new("/d/f"), Path::new("/d/sub/g")).is_none());
    }

    #[test]
    fn detects_identical_contained_and_similar() {
        let dir = std::env::temp_dir().join(format!("fd-analysis-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // Docs and Backup/PC/Docs are identical.
        for root in ["Docs", "Backup/PC/Docs"] {
            write(&dir, &format!("{root}/a.txt"), "alpha");
            write(&dir, &format!("{root}/sub/b.txt"), "beta");
        }
        // Old/Music is contained in Music.
        write(&dir, "Music/1.mp3", "one");
        write(&dir, "Music/2.mp3", "two");
        write(&dir, "Old/Music/1.mp3", "one");
        // Photos and USB/Photos share 2 of 3 files each.
        write(&dir, "Photos/p1.jpg", "p1");
        write(&dir, "Photos/p2.jpg", "p2");
        write(&dir, "Photos/p3.jpg", "p3");
        write(&dir, "USB/Photos/p1.jpg", "p1");
        write(&dir, "USB/Photos/p2.jpg", "p2");
        write(&dir, "USB/Photos/p4.jpg", "p4");

        let scan = scan_dir(&dir);
        let pairs = find_folder_pairs(&scan);
        let find = |x: &str, y: &str| {
            pairs
                .iter()
                .find(|p| {
                    let (a, b) = (dir.join(x), dir.join(y));
                    (p.a.path == a && p.b.path == b) || (p.a.path == b && p.b.path == a)
                })
                .unwrap_or_else(|| panic!("pair {x} / {y} not found in {pairs:#?}"))
        };

        assert_eq!(
            find("Docs", "Backup/PC/Docs").relation(),
            Relation::Identical
        );
        // The identical subfolder `sub` is covered by its parent pair.
        assert!(!pairs.iter().any(|p| p.a.path.ends_with("Docs/sub")));

        let music = find("Music", "Old/Music");
        let old = if music.a.path.ends_with("Old/Music") {
            &music.a
        } else {
            &music.b
        };
        assert_eq!(old.ratio(), 1.0);
        assert!(matches!(music.relation(), Relation::AInB | Relation::BInA));

        let photos = find("Photos", "USB/Photos");
        assert_eq!(photos.relation(), Relation::Similar);
        assert!((photos.a.ratio() - 2.0 / 3.0).abs() < 1e-6);

        let tree = CompareTree::build(&scan, photos);
        let root = &tree.nodes[0];
        assert_eq!(root.counts.same, 2);
        assert_eq!(root.counts.unique_a + root.counts.unique_b, 2);

        let _ = fs::remove_dir_all(dir);
    }
}
