use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn list_files_with_ignore(
    path: PathBuf,
    ignore_set: &HashSet<String>,
    min_size: u64,
) -> io::Result<Vec<PathBuf>> {
    let files: Vec<PathBuf> = WalkDir::new(&path)
        .into_iter()
        .filter_entry(|e| {
            if let Some(name) = e.file_name().to_str() {
                return !ignore_set.contains(name);
            }
            true
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| min_size == 0 || e.metadata().is_ok_and(|m| m.len() >= min_size))
        .map(|e| e.path().to_path_buf())
        .collect();

    Ok(files)
}

fn get_duplicated_files_by_byte(
    paths: Vec<PathBuf>,
    quick_scan: bool,
    on_progress: &mut dyn FnMut(),
) -> Vec<Vec<PathBuf>> {
    let mut hashes: Vec<u32> = Vec::new();
    let mut valid_paths: Vec<PathBuf> = Vec::new();
    let mut duplicated_files = Vec::new();

    for path in &paths {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(err) => {
                eprintln!("Warning: Could not open file ({}): {:?}", err, path);
                continue;
            }
        };
        let mut hasher = crc32fast::Hasher::new();
        if quick_scan {
            let mut buf = [0u8; 4096];
            match file.read(&mut buf) {
                Ok(_) => hasher.update(&buf),
                Err(err) => {
                    eprintln!("Warning: Could not read file ({}): {:?}", err, path);
                    continue;
                }
            }
        } else {
            let mut buf = [0u8; 8192];
            let mut success = true;
            loop {
                match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => hasher.update(&buf[..n]),
                    Err(err) => {
                        eprintln!("Warning: Could not read file ({}): {:?}", err, path);
                        success = false;
                        break;
                    }
                }
            }
            if !success {
                continue;
            }
        }
        hashes.push(hasher.finalize());
        valid_paths.push(path.clone());
        on_progress();
    }

    let mut is_duplicated = vec![false; hashes.len()];

    for (i, h1) in hashes.iter().enumerate() {
        if is_duplicated[i] {
            continue;
        }

        let mut equal_files = vec![valid_paths[i].clone()];
        for (j, h2) in hashes.iter().enumerate().skip(i + 1) {
            if h1 == h2 {
                is_duplicated[j] = true;
                equal_files.push(valid_paths[j].clone());
            }
        }

        if equal_files.len() > 1 {
            is_duplicated[i] = true;
            duplicated_files.push(equal_files);
        }
    }

    duplicated_files
}

pub fn group_files_by_size(paths: &[PathBuf]) -> HashMap<u64, Vec<PathBuf>> {
    let mut files_by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for p in paths {
        if let Ok(m) = p.metadata() {
            files_by_size.entry(m.len()).or_default().push(p.clone());
        }
    }
    files_by_size
}

pub fn get_duplicated_files(
    files_by_size: HashMap<u64, Vec<PathBuf>>,
    quick_scan: bool,
    mut on_progress: impl FnMut(),
) -> Vec<Vec<PathBuf>> {
    let mut duplicated_files: Vec<Vec<PathBuf>> = Vec::new();
    for (_, paths) in files_by_size {
        if paths.len() <= 1 {
            continue;
        }

        duplicated_files.extend(get_duplicated_files_by_byte(
            paths,
            quick_scan,
            &mut on_progress,
        ));
    }

    duplicated_files
}

pub struct DirectoryNode {
    pub path: PathBuf,
    pub files: Vec<(PathBuf, Vec<PathBuf>)>,
    pub children: Vec<DirectoryNode>,
}

impl DirectoryNode {
    pub fn total_count(&self) -> usize {
        self.files.len() + self.children.iter().map(|c| c.total_count()).sum::<usize>()
    }
}

pub fn build_directory_tree(root: &Path, duplicated_files: Vec<Vec<PathBuf>>) -> DirectoryNode {
    let mut file_to_group: HashMap<PathBuf, usize> = HashMap::new();
    let mut files_by_dir: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for (group_idx, group) in duplicated_files.iter().enumerate() {
        for file in group {
            file_to_group.insert(file.clone(), group_idx);

            if let Some(parent) = file.parent() {
                files_by_dir
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(file.clone());
            }
        }
    }

    fn build_node(
        path: &Path,
        files_by_dir: &HashMap<PathBuf, Vec<PathBuf>>,
        file_to_group: &HashMap<PathBuf, usize>,
        duplicated_files: &Vec<Vec<PathBuf>>,
    ) -> Option<DirectoryNode> {
        let mut child_dirs: HashSet<PathBuf> = HashSet::new();
        for dir in files_by_dir.keys() {
            if let Ok(rel) = dir.strip_prefix(path) {
                if rel.components().count() == 1 {
                    child_dirs.insert(dir.clone());
                }
            }
        }

        if child_dirs.is_empty() {
            let has_descendants = files_by_dir
                .keys()
                .any(|d| d.starts_with(path) && d != path);
            if has_descendants {
                for dir in files_by_dir.keys() {
                    if let Ok(rel) = dir.strip_prefix(path) {
                        if let Some(first) = rel.components().next() {
                            child_dirs.insert(path.join(first.as_os_str()));
                        }
                    }
                }
            }
        }

        let mut children: Vec<DirectoryNode> = child_dirs
            .iter()
            .filter_map(|d| build_node(d, files_by_dir, file_to_group, duplicated_files))
            .collect();
        children.sort_by(|a, b| a.path.cmp(&b.path));

        let files: Vec<(PathBuf, Vec<PathBuf>)> = files_by_dir
            .get(path)
            .map(|fs| {
                fs.iter()
                    .map(|f| {
                        let others = file_to_group
                            .get(f)
                            .and_then(|&idx| duplicated_files.get(idx))
                            .map(|group| group.iter().filter(|p| *p != f).cloned().collect())
                            .unwrap_or_default();
                        (f.clone(), others)
                    })
                    .collect()
            })
            .unwrap_or_default();

        if files.is_empty() && children.is_empty() {
            return None;
        }

        Some(DirectoryNode {
            path: path.to_path_buf(),
            files,
            children,
        })
    }

    build_node(root, &files_by_dir, &file_to_group, &duplicated_files).unwrap_or(DirectoryNode {
        path: root.to_path_buf(),
        files: Vec::new(),
        children: Vec::new(),
    })
}
