use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::PathBuf;

pub fn list_files(path: PathBuf) -> io::Result<Vec<PathBuf>> {
    list_files_with_ignore(path, &[])
}

pub fn list_files_with_ignore(path: PathBuf, ignore: &[String]) -> io::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path]);
    }

    let mut files = Vec::new();
    let entries = fs::read_dir(path)?;
    for entry in entries {
        let entry_path = entry?.path();
        if let Some(name) = entry_path.file_name() {
            let name_str = name.to_string_lossy();
            if ignore.iter().any(|p| name_str == p.as_str()) {
                continue;
            }
        }
        let nested_files = list_files_with_ignore(entry_path, ignore)?;
        files.extend(nested_files);
    }

    Ok(files)
}

fn get_duplicated_files_by_byte(paths: Vec<PathBuf>) -> Vec<Vec<PathBuf>> {
    const BUF_SIZE: usize = 1 << 12;

    let mut buf = [0; BUF_SIZE];
    let mut files = Vec::new();
    let mut duplicated_files = Vec::new();
    let mut is_duplicated = vec![false; paths.len()];

    for path in &paths {
        let mut file = File::open(path).unwrap();
        file.read(&mut buf).unwrap();
        files.push(buf.clone());
    }

    for (i, f1) in files.iter().enumerate() {
        if is_duplicated[i] {
            continue;
        }

        let mut equal_files = vec![paths[i].clone()];
        for (j, f2) in files.iter().enumerate().skip(i + 1) {
            if f1 == f2 {
                is_duplicated[j] = true;
                equal_files.push(paths[j].clone());
            }
        }

        if equal_files.len() > 1 {
            is_duplicated[i] = true;
            duplicated_files.push(equal_files);
        }
    }

    duplicated_files
}

pub fn get_duplicated_files(paths: Vec<PathBuf>) -> io::Result<Vec<Vec<PathBuf>>> {
    let mut map_by_len: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for path in paths {
        let len = path.metadata()?.len();
        map_by_len.entry(len).or_default().push(path);
    }

    let mut duplicated_files: Vec<Vec<PathBuf>> = Vec::new();
    for (_, paths) in map_by_len {
        if paths.len() <= 1 {
            continue;
        }

        duplicated_files.extend(get_duplicated_files_by_byte(paths));
    }

    Ok(duplicated_files)
}

fn sort_paths<'a>(p1: &'a PathBuf, p2: &'a PathBuf) -> (&'a PathBuf, &'a PathBuf) {
    if p1 < p2 {
        (p1, p2)
    } else {
        (p2, p1)
    }
}

pub fn get_shared_parents(
    duplicated_files: Vec<Vec<PathBuf>>,
) -> HashMap<(PathBuf, PathBuf), (Vec<PathBuf>, Vec<PathBuf>)> {
    let mut folders = HashMap::new();
    let mut duplicated_files_parents = Vec::with_capacity(duplicated_files.len());

    for same_files in &duplicated_files {
        let mut same_parents = Vec::with_capacity(same_files.len());
        for f in same_files {
            same_parents.push(f.parent().unwrap().to_path_buf());
        }
        duplicated_files_parents.push(same_parents);
    }

    for (i, same_parents) in duplicated_files_parents.iter().enumerate() {
        for (j, sp1) in same_parents.iter().enumerate() {
            for (k, sp2) in same_parents.iter().enumerate().skip(j + 1) {
                let (sp1, sp2) = sort_paths(sp1, sp2);

                let (files1, files2) = folders
                    .entry((sp1.to_path_buf(), sp2.to_path_buf()))
                    .or_insert((Vec::new(), Vec::new()));
                files1.push(duplicated_files[i][j].clone());
                files2.push(duplicated_files[i][k].clone());
            }
        }
    }

    folders
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

pub fn build_directory_tree(root: &PathBuf, duplicated_files: Vec<Vec<PathBuf>>) -> DirectoryNode {
    let mut file_to_others: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut files_by_dir: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for group in &duplicated_files {
        for file in group {
            let others: Vec<PathBuf> = group.iter().filter(|f| *f != file).cloned().collect();
            file_to_others.insert(file.clone(), others);

            if let Some(parent) = file.parent() {
                files_by_dir
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(file.clone());
            }
        }
    }

    fn build_node(
        path: &PathBuf,
        files_by_dir: &HashMap<PathBuf, Vec<PathBuf>>,
        file_to_others: &HashMap<PathBuf, Vec<PathBuf>>,
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
            .filter_map(|d| build_node(d, files_by_dir, file_to_others))
            .collect();
        children.sort_by(|a, b| a.path.cmp(&b.path));

        let files: Vec<(PathBuf, Vec<PathBuf>)> = files_by_dir
            .get(path)
            .map(|fs| {
                fs.iter()
                    .map(|f| {
                        let others = file_to_others.get(f).cloned().unwrap_or_default();
                        (f.clone(), others)
                    })
                    .collect()
            })
            .unwrap_or_default();

        if files.is_empty() && children.is_empty() {
            return None;
        }

        Some(DirectoryNode {
            path: path.clone(),
            files,
            children,
        })
    }

    build_node(root, &files_by_dir, &file_to_others).unwrap_or(DirectoryNode {
        path: root.clone(),
        files: Vec::new(),
        children: Vec::new(),
    })
}
