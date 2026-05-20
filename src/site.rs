use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use minijinja::{Environment, context};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::index::PackageIndex;

const MANIFEST_FILE_NAME: &str = "indexes.json";
const DATA_OUTPUT_DIR: &str = "data";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteManifest {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub indexes: Vec<String>,
    pub configs: Vec<String>,
}

pub struct SiteGenerator {
    title: String,
}

impl SiteGenerator {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }

    pub fn generate(&self, data_dir: impl AsRef<Path>, output_dir: impl AsRef<Path>) -> Result<()> {
        let data_dir = data_dir.as_ref();
        let output_dir = output_dir.as_ref();
        fs::create_dir_all(output_dir)
            .with_context(|| format!("creating site output directory {}", output_dir.display()))?;

        let package_indexes = find_package_indexes(data_dir)?;
        copy_data_dir(data_dir, &output_dir.join(DATA_OUTPUT_DIR))?;
        let manifest = build_manifest(data_dir, &package_indexes)?;

        let mut env = Environment::new();
        env.add_template("index.html", include_str!("templates/index.html"))
            .context("registering index.html template")?;

        let html = env
            .get_template("index.html")
            .context("loading index.html template")?
            .render(context! {
                title => self.title.as_str(),
                manifest_file => MANIFEST_FILE_NAME,
            })
            .context("rendering index.html")?;

        fs::write(output_dir.join("index.html"), html)
            .with_context(|| format!("writing {}", output_dir.join("index.html").display()))?;
        fs::write(output_dir.join("app.js"), include_str!("templates/app.js"))
            .with_context(|| format!("writing {}", output_dir.join("app.js").display()))?;
        fs::write(
            output_dir.join("styles.css"),
            include_str!("templates/styles.css"),
        )
        .with_context(|| format!("writing {}", output_dir.join("styles.css").display()))?;

        let manifest_json =
            serde_json::to_string_pretty(&manifest).context("serializing site manifest")?;
        fs::write(output_dir.join(MANIFEST_FILE_NAME), manifest_json).with_context(|| {
            format!("writing {}", output_dir.join(MANIFEST_FILE_NAME).display())
        })?;

        Ok(())
    }
}

pub fn find_package_indexes(data_dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let data_dir = data_dir.as_ref();
    let mut indexes = Vec::new();

    for entry in WalkDir::new(data_dir) {
        let entry = entry.with_context(|| format!("walking {}", data_dir.display()))?;
        if !entry.file_type().is_file() || entry.file_name() != "index.json" {
            continue;
        }

        read_package_index(entry.path())?;
        indexes.push(entry.path().to_path_buf());
    }

    indexes.sort();
    Ok(indexes)
}

fn build_manifest(data_dir: &Path, package_indexes: &[PathBuf]) -> Result<SiteManifest> {
    let mut indexes = Vec::with_capacity(package_indexes.len());
    let mut configs = BTreeSet::new();
    for index_path in package_indexes {
        let package_index = read_package_index(index_path)?;
        configs.extend(
            package_index
                .entries
                .keys()
                .map(|name| name.strip_prefix("CONFIG_").unwrap_or(name).to_string()),
        );

        let relative = index_path.strip_prefix(data_dir).with_context(|| {
            format!(
                "package index {} is not under {}",
                index_path.display(),
                data_dir.display()
            )
        })?;
        indexes.push(format!(
            "{DATA_OUTPUT_DIR}/{}",
            relative.to_string_lossy().replace('\\', "/")
        ));
    }

    Ok(SiteManifest {
        schema_version: 1,
        generated_at: Utc::now(),
        indexes,
        configs: configs.into_iter().collect(),
    })
}

fn read_package_index(index_path: &Path) -> Result<PackageIndex> {
    let json = fs::read_to_string(index_path)
        .with_context(|| format!("reading {}", index_path.display()))?;
    serde_json::from_str::<PackageIndex>(&json)
        .with_context(|| format!("parsing package index {}", index_path.display()))
}

fn copy_data_dir(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("creating data output directory {}", destination.display()))?;

    for entry in WalkDir::new(source) {
        let entry = entry.with_context(|| format!("walking {}", source.display()))?;
        let relative = entry.path().strip_prefix(source).with_context(|| {
            format!(
                "data path {} is not under {}",
                entry.path().display(),
                source.display()
            )
        })?;
        if relative.as_os_str().is_empty() {
            continue;
        }

        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("creating directory {}", target.display()))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating directory {}", parent.display()))?;
            }
            fs::copy(entry.path(), &target).with_context(|| {
                format!("copying {} to {}", entry.path().display(), target.display())
            })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Architecture, Distribution, write_packages_to_data_dir};
    use crate::indexer::KernelConfigPackage;

    #[test]
    fn writes_static_site_files_from_data_directory() {
        let data = tempfile::tempdir().expect("data tempdir");
        let site = tempfile::tempdir().expect("site tempdir");
        write_packages_to_data_dir(
            [KernelConfigPackage {
                distribution: Distribution::Debian,
                package_name: "linux-image-amd64".to_string(),
                package_version: "6.1.0-1".to_string(),
                architecture: Architecture::Amd64,
                source: None,
                config_text: "CONFIG_BPF=y\nCONFIG_EXT4_FS=m\n".to_string(),
            }],
            data.path(),
        )
        .expect("write data");

        SiteGenerator::new("kconfigwtf")
            .generate(data.path(), site.path())
            .expect("generate site");

        assert!(site.path().join("index.html").exists());
        assert!(site.path().join("app.js").exists());
        assert!(site.path().join("styles.css").exists());
        assert!(site.path().join("indexes.json").exists());
        let manifest: SiteManifest = serde_json::from_str(
            &fs::read_to_string(site.path().join("indexes.json")).expect("read manifest"),
        )
        .expect("parse manifest");
        assert_eq!(manifest.configs, vec!["BPF", "EXT4_FS"]);
        assert!(
            site.path()
                .join("data/debian/linux-image-amd64/index.json")
                .exists()
        );
        assert!(
            site.path()
                .join("data/debian/linux-image-amd64/6.1.0-1/amd64/config")
                .exists()
        );
    }
}
