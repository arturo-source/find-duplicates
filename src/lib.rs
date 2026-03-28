use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::PathBuf;

pub fn list_files(path: PathBuf) -> io::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path]);
    }

    let mut files = Vec::new();
    let entries = fs::read_dir(path)?;
    for entry in entries {
        let entry_path = entry?.path();
        let nested_files = list_files(entry_path)?;
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
    if p1 < p2 { (p1, p2) } else { (p2, p1) }
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

pub fn format_duplicates(path: PathBuf) -> io::Result<String> {
    let paths = list_files(path)?;
    let duplicated_files = get_duplicated_files(paths)?;
    let shared_parents = get_shared_parents(duplicated_files);

    let mut shared_parents_vec: Vec<_> = shared_parents.into_iter().collect();
    shared_parents_vec.sort_by(|a, b| b.1.0.len().cmp(&a.1.0.len()));

    let mut output = String::new();
    for ((parent1, parent2), (files1, files2)) in shared_parents_vec {
        output.push_str(&format!("In {parent1:?}:\n"));
        for f in files1 {
            output.push_str(&format!("  {:?}\n", f.file_name().unwrap()));
        }

        output.push_str(&format!("In {parent2:?}:\n"));
        for f in files2 {
            output.push_str(&format!("  {:?}\n", f.file_name().unwrap()));
        }

        output.push('\n');
    }

    Ok(output)
}
