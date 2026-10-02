//! Read-only occurrence storage rebuilt from the JSON indexes on every startup.
//!
//! Only lookup spans, kernel metadata, and shared values stay resident. Each disk
//! occurrence is a little-endian (kernel: u32, value: u32) pair. This is a private
//! temporary format, never a persistent replacement for the package indexes.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use super::{API_BASE, ConfigRecord, encode_url_path};
use crate::index::{
    Architecture, CompactPackageIndex, ConfigValue, PackageIndex, PackageKernel, read_package_index,
};
use crate::site::{SiteManifest, find_package_indexes};

struct Span {
    offset: u64,
    count: u32,
    package: u32,
}

struct Package {
    kernels: Vec<ConfigRecord>,
    values: Vec<String>,
}

pub(super) struct DiskLookup {
    #[cfg(unix)]
    file: File,
    #[cfg(not(unix))]
    file: std::sync::Mutex<File>,
    configs: BTreeMap<String, Box<[Span]>>,
    packages: Vec<Package>,
    pub manifest: SiteManifest,
    pub disk_bytes: u64,
}

impl DiskLookup {
    pub fn len(&self) -> usize {
        self.packages.len()
    }

    pub fn load(data_dir: &Path, cache_dir: &Path) -> Result<Self> {
        fs::create_dir_all(cache_dir)
            .with_context(|| format!("creating lookup directory {}", cache_dir.display()))?;
        let file = tempfile::tempfile_in(cache_dir).with_context(|| {
            format!("creating temporary lookup file in {}", cache_dir.display())
        })?;
        let mut builder = Builder {
            writer: BufWriter::new(file),
            offset: 0,
            configs: BTreeMap::new(),
            packages: Vec::new(),
            generated_at: None,
        };

        for path in find_package_indexes(data_dir)? {
            let relative_dir = path
                .parent()
                .context("package index has no parent directory")?
                .strip_prefix(data_dir)
                .with_context(|| format!("{} is outside the data directory", path.display()))?;
            let json = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            // Parsing the compact shape directly avoids Serde's untagged-enum
            // buffer and the expanded kernel String for every occurrence.
            let compact = serde_json::from_slice::<CompactPackageIndex>(&json);
            drop(json);
            let result = match compact {
                Ok(index) => builder.add_compact(relative_dir, index),
                Err(_) => read_package_index(&path)
                    .and_then(|index| builder.add_legacy(relative_dir, index)),
            };
            result.with_context(|| format!("building lookup from {}", path.display()))?;
        }
        builder.finish()
    }

    pub fn records_for_config(&self, name: &str) -> Result<Vec<ConfigRecord>> {
        let mut records = Vec::new();
        let Some(spans) = self.configs.get(name) else {
            return Ok(records);
        };
        let mut buffer = Vec::new();
        for span in spans.iter() {
            let package = &self.packages[span.package as usize];
            let start = records.len();
            records.extend(package.kernels.iter().cloned());
            let length = (span.count as usize)
                .checked_mul(8)
                .context("lookup span exceeds addressable memory")?;
            buffer.resize(length, 0);
            self.read_exact_at(&mut buffer, span.offset)
                .context("reading temporary lookup file")?;
            for occurrence in buffer.as_chunks::<8>().0 {
                let kernel = u32::from_le_bytes(occurrence[..4].try_into().unwrap()) as usize;
                let value = u32::from_le_bytes(occurrence[4..].try_into().unwrap()) as usize;
                let record = records[start..]
                    .get_mut(kernel)
                    .context("invalid kernel in temporary lookup file")?;
                record.value.clone_from(
                    package
                        .values
                        .get(value)
                        .context("invalid value in temporary lookup file")?,
                );
            }
        }
        records.sort_by(|left, right| {
            (
                &left.distribution,
                &left.release,
                &left.package_name,
                &left.version,
                &left.architecture,
            )
                .cmp(&(
                    &right.distribution,
                    &right.release,
                    &right.package_name,
                    &right.version,
                    &right.architecture,
                ))
        });
        Ok(records)
    }

    #[cfg(unix)]
    fn read_exact_at(&self, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
        use std::os::unix::fs::FileExt;
        self.file.read_exact_at(buffer, offset)
    }

    #[cfg(not(unix))]
    fn read_exact_at(&self, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = self
            .file
            .lock()
            .map_err(|_| std::io::Error::other("lookup file lock poisoned"))?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(buffer)
    }

    #[cfg(test)]
    pub(super) fn truncate(&self) {
        #[cfg(unix)]
        self.file.set_len(0).unwrap();
        #[cfg(not(unix))]
        self.file.lock().unwrap().set_len(0).unwrap();
    }
}

struct Builder {
    writer: BufWriter<File>,
    offset: u64,
    configs: BTreeMap<String, Vec<Span>>,
    packages: Vec<Package>,
    generated_at: Option<DateTime<Utc>>,
}

impl Builder {
    fn add_compact(&mut self, relative_dir: &Path, index: CompactPackageIndex) -> Result<()> {
        let mut kernel_ids = Vec::with_capacity(index.kernels.len());
        let mut kernels = BTreeMap::new();
        for kernel in index.kernels {
            let stored_architecture = index
                .architectures
                .get(kernel.architecture)
                .with_context(|| format!("invalid architecture index {}", kernel.architecture))?
                .clone();
            let architecture = stored_architecture
                .parse::<Architecture>()
                .map_err(anyhow::Error::msg)?;
            let release = index
                .releases
                .get(kernel.release)
                .with_context(|| format!("invalid release index {}", kernel.release))?
                .clone();
            let kernel_id = format!("{}/{}", kernel.version, stored_architecture);
            kernel_ids.push(kernel_id.clone());
            // Match the normal loader: metadata for duplicate IDs is last-wins,
            // and iteration follows lexicographic kernel-ID order.
            kernels.insert(
                kernel_id.clone(),
                record(
                    relative_dir,
                    index.distribution.as_str(),
                    &index.package_name,
                    PackageKernel {
                        version: kernel.version,
                        release,
                        architecture,
                        config_path: format!("{kernel_id}/config"),
                        source: kernel.source,
                        stored_architecture: Some(stored_architecture),
                    },
                ),
            );
        }
        let positions = kernel_positions(&kernels)?;
        let remap = kernel_ids
            .iter()
            .map(|id| positions[id.as_str()])
            .collect::<Vec<_>>();
        let mut values = ValueTable::new();
        for (name, entry) in index.entries {
            // Preserve original order for equal kernel IDs: built-in, module,
            // then other values. The last occurrence wins in the API. Explicit
            // missing lists are ignored, just as in the existing compact loader.
            let occurrences = entry
                .built_in
                .into_iter()
                .map(|kernel| (kernel, ConfigValue::BuiltIn))
                .chain(
                    entry
                        .module
                        .into_iter()
                        .map(|kernel| (kernel, ConfigValue::Module)),
                )
                .chain(
                    entry
                        .other
                        .into_iter()
                        .map(|(kernel, value)| (kernel, ConfigValue::Other(value))),
                )
                .map(|(kernel, value)| {
                    let kernel = *remap
                        .get(kernel)
                        .with_context(|| format!("invalid kernel index {kernel}"))?;
                    Ok((kernel, values.intern(value)?))
                });
            self.write_entry(name, occurrences)?;
        }
        self.add_package(index.generated_at, kernels.into_values().collect(), values);
        Ok(())
    }

    fn add_legacy(&mut self, relative_dir: &Path, index: PackageIndex) -> Result<()> {
        let kernels = index
            .kernels
            .into_iter()
            .map(|(id, kernel)| {
                (
                    id,
                    record(
                        relative_dir,
                        index.distribution.as_str(),
                        &index.package_name,
                        kernel,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let positions = kernel_positions(&kernels)?;
        let mut values = ValueTable::new();
        for (name, occurrences) in index.entries {
            // The original API ignores legacy occurrences for unknown kernels.
            let occurrences = occurrences.into_iter().filter_map(|occurrence| {
                positions
                    .get(occurrence.kernel.as_str())
                    .map(|&kernel| values.intern(occurrence.value).map(|value| (kernel, value)))
            });
            self.write_entry(name, occurrences)?;
        }
        self.add_package(index.generated_at, kernels.into_values().collect(), values);
        Ok(())
    }

    fn write_entry(
        &mut self,
        name: String,
        occurrences: impl Iterator<Item = Result<(u32, u32)>>,
    ) -> Result<()> {
        let mut span = Span {
            offset: self.offset,
            count: 0,
            package: self
                .packages
                .len()
                .try_into()
                .context("too many package indexes")?,
        };
        for occurrence in occurrences {
            let (kernel, value) = occurrence?;
            let mut bytes = [0; 8];
            bytes[..4].copy_from_slice(&kernel.to_le_bytes());
            bytes[4..].copy_from_slice(&value.to_le_bytes());
            self.writer
                .write_all(&bytes)
                .context("writing temporary lookup file")?;
            span.count = span
                .count
                .checked_add(1)
                .context("too many config occurrences")?;
            self.offset = self
                .offset
                .checked_add(8)
                .context("temporary lookup file too large")?;
        }
        self.configs.entry(name).or_default().push(span);
        Ok(())
    }

    fn add_package(
        &mut self,
        generated_at: DateTime<Utc>,
        kernels: Vec<ConfigRecord>,
        values: ValueTable,
    ) {
        self.generated_at = Some(
            self.generated_at
                .map_or(generated_at, |current| current.max(generated_at)),
        );
        self.packages.push(Package {
            kernels,
            values: values.strings,
        });
    }

    fn finish(self) -> Result<DiskLookup> {
        let file = self
            .writer
            .into_inner()
            .map_err(|error| error.into_error())
            .context("flushing temporary lookup file")?;
        let manifest = SiteManifest {
            schema_version: 1,
            generated_at: self.generated_at.unwrap_or_else(Utc::now),
            configs: self
                .configs
                .keys()
                .map(|name| name.strip_prefix("CONFIG_").unwrap_or(name).to_string())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        };
        Ok(DiskLookup {
            #[cfg(unix)]
            file,
            #[cfg(not(unix))]
            file: std::sync::Mutex::new(file),
            configs: self
                .configs
                .into_iter()
                .map(|(name, spans)| (name, spans.into_boxed_slice()))
                .collect(),
            packages: self.packages,
            manifest,
            disk_bytes: self.offset,
        })
    }
}

fn kernel_positions(kernels: &BTreeMap<String, ConfigRecord>) -> Result<BTreeMap<&str, u32>> {
    kernels
        .keys()
        .enumerate()
        .map(|(position, id)| {
            Ok((
                id.as_str(),
                position
                    .try_into()
                    .context("too many kernels in package index")?,
            ))
        })
        .collect()
}

fn record(
    relative_dir: &Path,
    distribution: &str,
    package_name: &str,
    kernel: PackageKernel,
) -> ConfigRecord {
    ConfigRecord {
        distribution: distribution.to_string(),
        release: kernel.release,
        package_name: package_name.to_string(),
        version: kernel.version,
        architecture: kernel
            .stored_architecture
            .unwrap_or_else(|| kernel.architecture.to_string()),
        value: ConfigValue::Missing.as_display_value().to_string(),
        source: kernel
            .source
            .filter(|source| source.starts_with("https://") || source.starts_with("http://")),
        config_url: format!(
            "{API_BASE}/raw/{}",
            encode_url_path(&relative_dir.join(kernel.config_path).to_string_lossy())
        ),
    }
}

struct ValueTable {
    strings: Vec<String>,
    other: BTreeMap<String, u32>,
}

impl ValueTable {
    fn new() -> Self {
        Self {
            strings: vec!["y".to_string(), "m".to_string()],
            other: BTreeMap::new(),
        }
    }

    fn intern(&mut self, value: ConfigValue) -> Result<u32> {
        match value {
            ConfigValue::BuiltIn => Ok(0),
            ConfigValue::Module => Ok(1),
            value => {
                let value = value.as_display_value();
                if let Some(&id) = self.other.get(value) {
                    return Ok(id);
                }
                let id = self
                    .strings
                    .len()
                    .try_into()
                    .context("too many config values")?;
                self.strings.push(value.to_string());
                self.other.insert(value.to_string(), id);
                Ok(id)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn compact_fixture() -> Value {
        json!({
            "schema_version": 6,
            "generated_at": "2026-01-02T00:00:00Z",
            "distribution": "fedora",
            "package_name": "kernel-<VERSION>",
            "releases": ["old", "new"],
            "architectures": ["ppc64le", "x86_64"],
            "kernels": [
                {"version": "6.9", "release": 0, "architecture": 0},
                {"version": "6.10", "release": 0, "architecture": 1,
                 "source": "javascript:invalid"},
                {"version": "6.9", "release": 1, "architecture": 0,
                 "source": "https://example.test/kernel"}
            ],
            "entries": {
                "CONFIG_DUPLICATE": {"built_in": [0, 1, 0], "module": [2, 1],
                    "other": [[0, "first"], [2, "last"], [2, "last"]]},
                "CONFIG_MISSING": {"missing": [0, 1, 999]},
                "CONFIG_EMPTY": {},
                "CONFIG_SPARSE": {"built_in": [2]},
                "CONFIG_TEXT": {"other": [[1, "\"a\\b\""], [0, "-"], [2, "y"]]},
                "CONFIG_Y": {"built_in": [1]},
                "CONFIG_M": {"module": [1]}
            }
        })
    }

    fn legacy_fixture() -> Value {
        json!({
            "schema_version": 5,
            "generated_at": "2026-01-01T00:00:00Z",
            "distribution": "fedora",
            "package_name": "kernel-<VERSION>",
            "kernels": {
                "6.8/ppc64le": {"version": "6.8", "architecture": "ppc64el",
                    "config_path": "6.8/ppc64le/config", "source": "http://example.test/kernel"},
                "6.7/x86_64": {"version": "6.7", "release": "old", "architecture": "amd64",
                    "config_path": "custom/path/config"}
            },
            "entries": {
                "CONFIG_DUPLICATE": [
                    {"kernel": "6.8/ppc64le", "value": "built_in"},
                    {"kernel": "6.8/ppc64le", "value": "module"},
                    {"kernel": "6.8/ppc64le", "value": {"other": "last"}},
                    {"kernel": "unknown", "value": "built_in"}
                ],
                "CONFIG_MISSING": [{"kernel": "6.8/ppc64le", "value": "-"}],
                "CONFIG_EMPTY": []
            }
        })
    }

    fn write_index(data: &Path, name: &str, index: &Value) -> std::path::PathBuf {
        let package_dir = data.join("fedora/kernel-<VERSION>");
        fs::create_dir_all(&package_dir).unwrap();
        let path = package_dir.join(name);
        fs::write(&path, serde_json::to_vec(index).unwrap()).unwrap();
        path
    }

    // The previous expanded-index lookup is an independent compatibility oracle.
    fn original_records(data: &Path, config: &str) -> Vec<ConfigRecord> {
        let mut records = Vec::new();
        for path in find_package_indexes(data).unwrap() {
            let relative_dir = path.parent().unwrap().strip_prefix(data).unwrap();
            let index = read_package_index(&path).unwrap();
            let Some(occurrences) = index.entries.get(config) else {
                continue;
            };
            let by_kernel = occurrences
                .iter()
                .map(|item| (item.kernel.as_str(), &item.value))
                .collect::<BTreeMap<_, _>>();
            for (id, kernel) in index.kernels {
                let raw_path = relative_dir.join(&kernel.config_path);
                records.push(ConfigRecord {
                    distribution: index.distribution.to_string(),
                    release: kernel.release,
                    package_name: index.package_name.clone(),
                    version: kernel.version,
                    architecture: kernel
                        .stored_architecture
                        .unwrap_or_else(|| kernel.architecture.to_string()),
                    value: by_kernel
                        .get(id.as_str())
                        .map(|value| value.as_display_value())
                        .unwrap_or("-")
                        .to_string(),
                    source: kernel.source.filter(|source| {
                        source.starts_with("https://") || source.starts_with("http://")
                    }),
                    config_url: format!(
                        "{API_BASE}/raw/{}",
                        encode_url_path(&raw_path.to_string_lossy())
                    ),
                });
            }
        }
        records.sort_by(|a, b| {
            (
                &a.distribution,
                &a.release,
                &a.package_name,
                &a.version,
                &a.architecture,
            )
                .cmp(&(
                    &b.distribution,
                    &b.release,
                    &b.package_name,
                    &b.version,
                    &b.architecture,
                ))
        });
        records
    }

    #[test]
    fn matches_expanded_lookup_for_compact_legacy_and_sharded_indexes() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        write_index(data.path(), "index.json", &compact_fixture());
        write_index(data.path(), "index_legacy.json", &legacy_fixture());
        let lookup = DiskLookup::load(data.path(), cache.path()).unwrap();
        assert_eq!(lookup.len(), 2);
        assert_eq!(
            lookup.manifest.generated_at.to_rfc3339(),
            "2026-01-02T00:00:00+00:00"
        );
        assert_eq!(
            lookup.manifest.configs,
            ["DUPLICATE", "EMPTY", "M", "MISSING", "SPARSE", "TEXT", "Y"]
        );
        for config in &lookup.manifest.configs {
            let name = format!("CONFIG_{config}");
            assert_eq!(
                lookup.records_for_config(&name).unwrap(),
                original_records(data.path(), &name),
                "{name}"
            );
        }
        assert!(
            lookup
                .records_for_config("CONFIG_UNKNOWN")
                .unwrap()
                .is_empty()
        );
        let sparse = lookup.records_for_config("CONFIG_SPARSE").unwrap();
        assert_eq!(
            sparse
                .iter()
                .map(|record| record.value.as_str())
                .collect::<Vec<_>>(),
            ["y", "-"]
        );
        assert_eq!(sparse[0].architecture, "ppc64le");
        assert!(sparse[0].config_url.contains("ppc64le"));
    }

    #[test]
    fn snapshot_survives_source_changes_and_restart_rebuilds_it() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = write_index(data.path(), "index.json", &compact_fixture());
        let lookup = DiskLookup::load(data.path(), cache.path()).unwrap();
        let before = lookup.records_for_config("CONFIG_Y").unwrap();
        let mut changed = compact_fixture();
        changed["entries"]["CONFIG_Y"] = json!({"module": [1]});
        fs::write(path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_eq!(lookup.records_for_config("CONFIG_Y").unwrap(), before);
        let reloaded = DiskLookup::load(data.path(), cache.path()).unwrap();
        assert_ne!(reloaded.records_for_config("CONFIG_Y").unwrap(), before);
        drop(lookup);
        drop(reloaded);
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
    }

    #[test]
    fn validates_compact_references_and_reports_the_source_file() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        for (pointer, invalid) in [
            ("/kernels/0/release", json!(999)),
            ("/kernels/0/architecture", json!(999)),
            ("/architectures/0", json!("")),
            ("/entries/CONFIG_Y/built_in/0", json!(999)),
            ("/entries/CONFIG_M/module/0", json!(999)),
            ("/entries/CONFIG_TEXT/other/0/0", json!(999)),
        ] {
            let mut fixture = compact_fixture();
            *fixture.pointer_mut(pointer).unwrap() = invalid;
            let path = write_index(data.path(), "index.json", &fixture);
            let error = DiskLookup::load(data.path(), cache.path())
                .err()
                .expect("invalid index must fail");
            assert!(
                format!("{error:#}").contains(&path.display().to_string()),
                "{error:#}"
            );
        }
    }

    #[test]
    fn supports_empty_data_and_reports_cache_and_read_failures() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let empty = DiskLookup::load(data.path(), cache.path()).unwrap();
        assert!(empty.manifest.configs.is_empty());
        assert_eq!(empty.disk_bytes, 0);
        write_index(data.path(), "index.json", &compact_fixture());
        let not_directory = cache.path().join("file");
        fs::write(&not_directory, b"x").unwrap();
        assert!(DiskLookup::load(data.path(), &not_directory).is_err());
        let lookup = DiskLookup::load(data.path(), cache.path()).unwrap();
        lookup.truncate();
        let error = lookup.records_for_config("CONFIG_Y").unwrap_err();
        assert!(format!("{error:#}").contains("reading temporary lookup file"));
    }
}
