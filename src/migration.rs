use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use walkdir::WalkDir;

use crate::index::{
    PackageIndex, is_package_index_file_name, list_package_index_files, read_package_index,
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

        drop_unknown_release_kernels(package_dir, &mut index)?;
        index_files_written.extend(write_package_index_to_dir(&index, package_dir, max_bytes)?);
    }

    index_files_written.sort();
    Ok(MigrationSummary {
        package_dirs: package_dirs.len(),
        index_files_written,
    })
}

fn drop_unknown_release_kernels(package_dir: &Path, index: &mut PackageIndex) -> Result<()> {
    let unknown_kernels = index
        .kernels
        .iter()
        .filter(|(_, kernel)| kernel.release == "unknown")
        .map(|(kernel_id, kernel)| (kernel_id.clone(), kernel.config_path.clone()))
        .collect::<Vec<_>>();

    for (kernel_id, config_path) in unknown_kernels {
        remove_kernel_config_file(package_dir, &config_path)?;
        index.remove_kernel(&kernel_id);
    }

    Ok(())
}

fn remove_kernel_config_file(package_dir: &Path, config_path: &str) -> Result<()> {
    let relative_path = Path::new(config_path);
    if relative_path.is_absolute() {
        bail!(
            "kernel config path {} must be relative to {}",
            relative_path.display(),
            package_dir.display()
        );
    }

    let full_path = package_dir.join(relative_path);
    if full_path.exists() {
        fs::remove_file(&full_path)
            .with_context(|| format!("removing kernel config {}", full_path.display()))?;
        prune_empty_parent_dirs(package_dir, &full_path)?;
    }

    Ok(())
}

fn prune_empty_parent_dirs(package_dir: &Path, removed_file: &Path) -> Result<()> {
    let mut current = removed_file.parent();
    while let Some(dir) = current {
        if dir == package_dir {
            break;
        }

        let mut entries =
            fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?;
        if entries.next().is_some() {
            break;
        }

        fs::remove_dir(dir).with_context(|| format!("removing directory {}", dir.display()))?;
        current = dir.parent();
    }

    Ok(())
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
