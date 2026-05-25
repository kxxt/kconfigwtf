use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::index::{
    is_package_index_file_name, list_package_index_files, read_package_index,
    write_package_index_to_dir,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationSummary {
    pub package_dirs: usize,
    pub index_files_written: Vec<PathBuf>,
}

pub fn migrate_data_dir(data_dir: &Path, max_bytes: usize) -> Result<MigrationSummary> {
    let package_dirs = find_package_dirs(data_dir)?;
    let mut index_files_written = Vec::new();

    for package_dir in &package_dirs {
        let mut index_paths = list_package_index_files(package_dir)?;
        if index_paths.is_empty() {
            continue;
        }

        index_paths.sort();
        let first = index_paths.remove(0);
        let mut index = read_package_index(&first)
            .with_context(|| format!("loading package index {}", first.display()))?;
        for path in index_paths {
            let shard = read_package_index(&path)
                .with_context(|| format!("loading package index shard {}", path.display()))?;
            index.merge(shard)?;
        }

        index_files_written.extend(write_package_index_to_dir(&index, package_dir, max_bytes)?);
    }

    index_files_written.sort();
    Ok(MigrationSummary {
        package_dirs: package_dirs.len(),
        index_files_written,
    })
}

fn find_package_dirs(data_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut package_dirs = BTreeSet::new();

    for entry in WalkDir::new(data_dir) {
        let entry = entry.with_context(|| format!("walking {}", data_dir.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }

        let Some(name) = entry.file_name().to_str() else {
            continue;
        };
        if !is_package_index_file_name(name) {
            continue;
        }

        if let Some(parent) = entry.path().parent() {
            package_dirs.insert(parent.to_path_buf());
        }
    }

    Ok(package_dirs.into_iter().collect())
}
