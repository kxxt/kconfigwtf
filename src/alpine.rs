use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use flate2::read::MultiGzDecoder;
use tar::Archive;

use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage, normalize_alpine_release_label};

const DEFAULT_PACKAGE_PREFIX: &str = "linux-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApkIndexLocation {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApkPackageBase {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlpineRepoFeed {
    pub distribution: Distribution,
    pub architecture: Architecture,
    pub index: ApkIndexLocation,
    pub package_base: ApkPackageBase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlpineIndexerConfig {
    pub release: String,
    pub feeds: Vec<AlpineRepoFeed>,
    pub package_name_prefix: String,
    pub max_packages: Option<usize>,
}

impl AlpineIndexerConfig {
    pub fn from_mirror(
        distribution: Distribution,
        mirror: impl Into<String>,
        release: impl AsRef<str>,
        repository: impl AsRef<str>,
        architectures: impl IntoIterator<Item = Architecture>,
    ) -> Self {
        let mirror = mirror.into().trim_end_matches('/').to_string();
        let release = release.as_ref();
        let repository = repository.as_ref();
        let feeds = architectures
            .into_iter()
            .map(|architecture| {
                let repo_root = alpine_repo_root(&mirror, release, repository, &architecture);
                AlpineRepoFeed {
                    distribution: distribution.clone(),
                    architecture,
                    index: ApkIndexLocation::Url(format!("{repo_root}/APKINDEX.tar.gz")),
                    package_base: ApkPackageBase::Url(repo_root),
                }
            })
            .collect();

        Self {
            release: normalize_alpine_release_label(release),
            feeds,
            package_name_prefix: DEFAULT_PACKAGE_PREFIX.to_string(),
            max_packages: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AlpineIndexer {
    config: AlpineIndexerConfig,
    client: reqwest::Client,
}

impl AlpineIndexer {
    pub fn new(config: AlpineIndexerConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    async fn load_index(&self, location: &ApkIndexLocation) -> Result<(String, Vec<u8>)> {
        match location {
            ApkIndexLocation::Url(url) => {
                let response = self
                    .client
                    .get(url)
                    .send()
                    .await
                    .with_context(|| format!("requesting APKINDEX {url}"))?
                    .error_for_status()
                    .with_context(|| format!("APKINDEX returned an error: {url}"))?;
                let bytes = response
                    .bytes()
                    .await
                    .with_context(|| format!("reading APKINDEX {url}"))?;
                Ok((url.clone(), bytes.to_vec()))
            }
            ApkIndexLocation::Path(path) => {
                let bytes = tokio::fs::read(path)
                    .await
                    .with_context(|| format!("reading APKINDEX {}", path.display()))?;
                Ok((path.display().to_string(), bytes))
            }
        }
    }

    async fn load_package(
        &self,
        base: &ApkPackageBase,
        filename: &str,
    ) -> Result<(String, Vec<u8>)> {
        match base {
            ApkPackageBase::Url(base_url) => {
                let url = join_url(base_url, filename);
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("requesting apk package {url}"))?
                    .error_for_status()
                    .with_context(|| format!("apk package returned an error: {url}"))?;
                let bytes = response
                    .bytes()
                    .await
                    .with_context(|| format!("reading apk package {url}"))?;
                Ok((url, bytes.to_vec()))
            }
            ApkPackageBase::Path(root) => {
                let path = root.join(filename);
                let bytes = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("reading apk package {}", path.display()))?;
                Ok((path.display().to_string(), bytes))
            }
        }
    }
}

#[async_trait]
impl KernelConfigIndexer for AlpineIndexer {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>> {
        let mut packages = Vec::new();
        let mut selected_package_count = 0usize;

        for feed in &self.config.feeds {
            let (index_source, index_bytes) = self.load_index(&feed.index).await?;
            let candidates = select_kernel_packages(
                &parse_apkindex(&index_bytes)
                    .with_context(|| format!("parsing APKINDEX {index_source}"))?,
                &self.config.package_name_prefix,
                Some(feed.architecture.clone()),
                self.config.max_packages,
            );
            selected_package_count += candidates.len();

            for candidate in candidates {
                let (source, package_bytes) = self
                    .load_package(&feed.package_base, &candidate.filename)
                    .await?;
                let configs = extract_kernel_configs_from_apk(&package_bytes)
                    .with_context(|| format!("extracting kernel config from {source}"))?;

                for (config_path, config_text) in configs {
                    packages.push(KernelConfigPackage {
                        distribution: feed.distribution.clone(),
                        release: self.config.release.clone(),
                        package_name: candidate.name.clone(),
                        package_version: candidate.version.clone(),
                        architecture: candidate.architecture.clone(),
                        source: Some(format!("{source}#{config_path}")),
                        config_text,
                    });
                }
            }
        }

        if selected_package_count == 0 {
            bail!(
                "Alpine indexer did not find any packages matching prefix {:?}",
                self.config.package_name_prefix
            );
        }

        if packages.is_empty() {
            bail!(
                "Alpine indexer selected {selected_package_count} package(s), but none contained a kernel config"
            );
        }

        Ok(packages)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApkPackageCandidate {
    pub name: String,
    pub version: String,
    pub architecture: Architecture,
    pub filename: String,
}

pub fn parse_apkindex(bytes: &[u8]) -> Result<Vec<ApkPackageCandidate>> {
    let mut apkindex = String::new();
    let decoder = MultiGzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(decoder);

    for entry in archive.entries().context("reading APKINDEX.tar.gz")? {
        let mut entry = entry.context("reading APKINDEX.tar.gz entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().context("reading APKINDEX entry path")?;
        if path.file_name().and_then(|name| name.to_str()) != Some("APKINDEX") {
            continue;
        }
        entry
            .read_to_string(&mut apkindex)
            .context("reading APKINDEX file")?;
        return parse_apkindex_text(&apkindex);
    }

    bail!("APKINDEX.tar.gz did not contain an APKINDEX file")
}

pub fn parse_apkindex_text(input: &str) -> Result<Vec<ApkPackageCandidate>> {
    input
        .split("\n\n")
        .filter_map(parse_apkindex_record)
        .collect()
}

fn parse_apkindex_record(record: &str) -> Option<Result<ApkPackageCandidate>> {
    let name = apk_field(record, "P:")?;
    let version = apk_field(record, "V:")?;
    let architecture = apk_field(record, "A:")?;
    let filename = format!("{name}-{version}.apk");

    Some(Ok(ApkPackageCandidate {
        name: name.to_string(),
        version: version.to_string(),
        architecture: architecture.parse().map_err(anyhow::Error::msg).ok()?,
        filename,
    }))
}

fn apk_field<'a>(record: &'a str, prefix: &str) -> Option<&'a str> {
    record.lines().find_map(|line| line.strip_prefix(prefix))
}

pub fn select_kernel_packages(
    packages: &[ApkPackageCandidate],
    package_name_prefix: &str,
    architecture: Option<Architecture>,
    max_packages: Option<usize>,
) -> Vec<ApkPackageCandidate> {
    let mut selected = packages
        .iter()
        .filter(|package| package.name.starts_with(package_name_prefix))
        .filter(|package| is_alpine_kernel_package(&package.name))
        .filter(|package| {
            architecture
                .as_ref()
                .is_none_or(|architecture| &package.architecture == architecture)
        })
        .cloned()
        .collect::<Vec<_>>();

    selected.sort_by(|left, right| {
        (&left.name, &left.version, &left.architecture).cmp(&(
            &right.name,
            &right.version,
            &right.architecture,
        ))
    });

    if let Some(max) = max_packages {
        selected.truncate(max);
    }

    selected
}

pub fn extract_kernel_configs_from_apk(bytes: &[u8]) -> Result<Vec<(String, String)>> {
    let decoder = MultiGzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(decoder);
    let mut configs = Vec::new();

    for entry in archive.entries().context("reading apk package entries")? {
        let mut entry = entry.context("reading apk package entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry.path().context("reading apk package entry path")?;
        if !is_kernel_config_path(&path) {
            continue;
        }

        let path = path.display().to_string();
        let mut config_text = String::new();
        entry
            .read_to_string(&mut config_text)
            .with_context(|| format!("reading kernel config {path}"))?;
        configs.push((path, config_text));
    }

    configs.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(configs)
}

fn is_alpine_kernel_package(name: &str) -> bool {
    !name.starts_with("linux-firmware")
        && name != "linux-headers"
        && !name.starts_with("linux-tools")
        && !name.starts_with("linux-pam")
        && !name.ends_with("-dev")
        && !name.ends_with("-doc")
        && !name.ends_with("-dbg")
}

fn is_kernel_config_path(path: &Path) -> bool {
    let normalized = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();

    if normalized.len() >= 2
        && normalized[normalized.len() - 2] == "boot"
        && normalized[normalized.len() - 1].starts_with("config-")
    {
        return true;
    }

    normalized.len() >= 3
        && normalized[0] == "usr"
        && normalized[1] == "src"
        && normalized[normalized.len() - 1] == ".config"
}

fn alpine_repo_root(
    mirror: &str,
    release: &str,
    repository: &str,
    architecture: &Architecture,
) -> String {
    format!(
        "{mirror}/{release}/{repository}/{}",
        apk_architecture_segment(architecture)
    )
}

fn apk_architecture_segment(architecture: &Architecture) -> &str {
    match architecture {
        Architecture::Amd64 => "x86_64",
        Architecture::Arm64 => "aarch64",
        Architecture::Armhf => "armv7",
        Architecture::I386 => "x86",
        Architecture::Ppc64el => "ppc64le",
        other => other.as_str(),
    }
}

fn join_url(base: &str, filename: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        filename.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;
    use tar::{Builder, Header};

    #[test]
    fn parses_and_selects_apkindex_kernel_packages() {
        let packages = parse_apkindex_text(
            r#"P:linux-lts
V:6.18.32-r0
A:x86_64

P:linux-lts-dev
V:6.18.32-r0
A:x86_64

P:linux-stable
V:7.0.9-r0
A:x86_64

P:busybox
V:1.37.0-r0
A:x86_64
"#,
        )
        .expect("parse apkindex");
        let selected = select_kernel_packages(&packages, "linux-", Some(Architecture::Amd64), None);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].name, "linux-lts");
        assert_eq!(selected[0].filename, "linux-lts-6.18.32-r0.apk");
        assert_eq!(selected[1].name, "linux-stable");
        assert_eq!(selected[1].filename, "linux-stable-7.0.9-r0.apk");
    }

    #[test]
    fn extracts_boot_config_from_apk() {
        let apk = apk_with_file("boot/config-6.18.32-0-lts", b"CONFIG_BPF=y\n");

        let configs = extract_kernel_configs_from_apk(&apk).expect("extract config");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "boot/config-6.18.32-0-lts");
        assert!(configs[0].1.contains("CONFIG_BPF=y"));
    }

    #[test]
    fn builds_alpine_repository_urls() {
        let config = AlpineIndexerConfig::from_mirror(
            Distribution::Alpine,
            "https://example.invalid/alpine",
            "latest-stable",
            "main",
            [Architecture::Amd64],
        );
        let url = match &config.feeds[0].index {
            ApkIndexLocation::Url(url) => url,
            ApkIndexLocation::Path(_) => panic!("expected url"),
        };

        assert_eq!(
            url,
            "https://example.invalid/alpine/latest-stable/main/x86_64/APKINDEX.tar.gz"
        );
    }

    fn apk_with_file(path: &str, contents: &[u8]) -> Vec<u8> {
        let tarball = tar_with_file(path, contents);
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&tarball).expect("write gzip");
        gz.finish().expect("finish gzip")
    }

    fn tar_with_file(path: &str, contents: &[u8]) -> Vec<u8> {
        let mut tarball = Vec::new();
        {
            let mut builder = Builder::new(&mut tarball);
            let mut header = Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, path, contents)
                .expect("append file");
            builder.finish().expect("finish tar");
        }
        tarball
    }
}
