//! Measurement prototype. Current compact indexes only; not production code.
use crate::server::ConfigRecord;
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use kconfigwtf::{
    index::Distribution,
    site::{SiteManifest, find_package_indexes},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufWriter, Write},
    os::unix::fs::FileExt,
    path::Path,
};

#[derive(Deserialize)]
struct Kernel {
    version: String,
    release: usize,
    architecture: usize,
    source: Option<String>,
}
#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    built_in: Box<[u32]>,
    #[serde(default)]
    module: Box<[u32]>,
    #[serde(default)]
    other: Box<[(u32, String)]>,
}
#[derive(Deserialize)]
struct Index {
    distribution: Distribution,
    package_name: String,
    generated_at: DateTime<Utc>,
    releases: Vec<String>,
    architectures: Vec<String>,
    kernels: Vec<Kernel>,
    entries: BTreeMap<String, Entry>,
}
struct Span {
    offset: u64,
    count: u32,
    package: u32,
}
struct Package {
    kernels: Vec<ConfigRecord>,
    values: Vec<String>,
}
pub struct DiskLookup {
    file: File,
    configs: BTreeMap<String, Box<[Span]>>,
    packages: Vec<Package>,
    pub manifest: SiteManifest,
}
impl DiskLookup {
    pub fn len(&self) -> usize {
        self.packages.len()
    }
    pub fn load(data_dir: &Path) -> Result<Self> {
        // Deliberately use the workspace filesystem: /tmp on this host is tmpfs.
        let cache_dir = Path::new("devlog/2026/10/02/backend-memory/cache");
        fs::create_dir_all(cache_dir)?;
        let file = tempfile::tempfile_in(cache_dir)?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, &file);
        let mut configs = BTreeMap::<String, Vec<Span>>::new();
        let mut packages = Vec::new();
        let mut generated_at = None;
        let mut offset = 0u64;
        for path in find_package_indexes(data_dir)? {
            let json = fs::read(&path)?;
            let index: Index = serde_json::from_slice(&json)
                .with_context(|| format!("parsing {}", path.display()))?;
            drop(json);
            generated_at = Some(generated_at.map_or(index.generated_at, |current| {
                std::cmp::max(current, index.generated_at)
            }));
            let relative_dir = path.parent().unwrap().strip_prefix(data_dir)?;
            let mut kernels = Vec::new();
            for kernel in index.kernels {
                let architecture = index
                    .architectures
                    .get(kernel.architecture)
                    .context("invalid architecture index")?
                    .clone();
                let release = index
                    .releases
                    .get(kernel.release)
                    .context("invalid release index")?
                    .clone();
                let raw_path = relative_dir
                    .join(&kernel.version)
                    .join(&architecture)
                    .join("config");
                let encoded_path = raw_path
                    .to_string_lossy()
                    .split('/')
                    .map(|part| {
                        percent_encoding::utf8_percent_encode(
                            part,
                            percent_encoding::NON_ALPHANUMERIC,
                        )
                        .to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                kernels.push(ConfigRecord {
                    distribution: index.distribution.to_string(),
                    package_name: index.package_name.clone(),
                    release,
                    architecture,
                    version: kernel.version,
                    source: kernel.source.filter(|source| {
                        source.starts_with("http://") || source.starts_with("https://")
                    }),
                    config_url: format!("/api/v1/raw/{encoded_path}"),
                    value: "-".into(),
                });
            }
            let mut values = vec!["y".to_string(), "m".to_string()];
            let mut value_ids = BTreeMap::<String, u32>::new();
            for (name, entry) in index.entries {
                let count = entry.built_in.len() + entry.module.len() + entry.other.len();
                configs.entry(name).or_default().push(Span {
                    offset,
                    count: count.try_into()?,
                    package: packages.len().try_into()?,
                });
                let mut write_pair = |kernel: u32, value: u32| -> Result<()> {
                    ensure!(
                        (kernel as usize) < kernels.len(),
                        "invalid kernel index {kernel}"
                    );
                    writer.write_all(&kernel.to_le_bytes())?;
                    writer.write_all(&value.to_le_bytes())?;
                    offset += 8;
                    Ok(())
                };
                for kernel in entry.built_in {
                    write_pair(kernel, 0)?;
                }
                for kernel in entry.module {
                    write_pair(kernel, 1)?;
                }
                for (kernel, value) in entry.other {
                    let id = if let Some(&id) = value_ids.get(&value) {
                        id
                    } else {
                        let id = values.len().try_into()?;
                        value_ids.insert(value.clone(), id);
                        values.push(value);
                        id
                    };
                    write_pair(kernel, id)?;
                }
            }
            packages.push(Package { kernels, values });
        }
        writer.flush()?;
        drop(writer);
        let manifest = SiteManifest {
            schema_version: 1,
            generated_at: generated_at.unwrap_or_else(Utc::now),
            configs: configs
                .keys()
                .map(|name| name.strip_prefix("CONFIG_").unwrap_or(name).to_string())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
        };
        let configs = configs
            .into_iter()
            .map(|(name, spans)| (name, spans.into_boxed_slice()))
            .collect();
        eprintln!(
            "temporary occurrence file: {offset} bytes; {} package indexes",
            packages.len()
        );
        Ok(Self {
            file,
            configs,
            packages,
            manifest,
        })
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
            buffer.resize(span.count as usize * 8, 0);
            self.file.read_exact_at(&mut buffer, span.offset)?;
            for occurrence in buffer.chunks_exact(8) {
                let kernel = u32::from_le_bytes(occurrence[..4].try_into().unwrap()) as usize;
                let value = u32::from_le_bytes(occurrence[4..].try_into().unwrap()) as usize;
                records[start + kernel]
                    .value
                    .clone_from(&package.values[value]);
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
        Ok(records)
    }
}
