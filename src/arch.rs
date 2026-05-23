use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use flate2::read::GzDecoder;
use liblzma::read::XzDecoder;
use tar::Archive;

use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage};

const DEFAULT_PACKAGE_PREFIX: &str = "linux";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchDatabaseLocation {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchPackageBase {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchRepositoryLayout {
    RepoOsArch,
    RepoArch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchRepoFeed {
    pub distribution: Distribution,
    pub architecture: Architecture,
    pub database: ArchDatabaseLocation,
    pub package_base: ArchPackageBase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchIndexerConfig {
    pub feeds: Vec<ArchRepoFeed>,
    pub package_name_prefix: String,
    pub max_packages: Option<usize>,
}

impl ArchIndexerConfig {
    pub fn from_mirror(
        distribution: Distribution,
        mirror: impl Into<String>,
        repository: impl AsRef<str>,
        architectures: impl IntoIterator<Item = Architecture>,
    ) -> Self {
        let mirror = mirror.into().trim_end_matches('/').to_string();
        let repository = repository.as_ref();
        let layout = default_layout(&distribution);
        let feeds = architectures
            .into_iter()
            .map(|architecture| {
                let repo_root = arch_repo_root(&mirror, repository, &architecture, layout);
                ArchRepoFeed {
                    distribution: distribution.clone(),
                    architecture,
                    database: ArchDatabaseLocation::Url(format!("{repo_root}/{repository}.db")),
                    package_base: ArchPackageBase::Url(repo_root),
                }
            })
            .collect();

        Self {
            feeds,
            package_name_prefix: DEFAULT_PACKAGE_PREFIX.to_string(),
            max_packages: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArchIndexer {
    config: ArchIndexerConfig,
    client: reqwest::Client,
}

impl ArchIndexer {
    pub fn new(config: ArchIndexerConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    async fn load_database(&self, location: &ArchDatabaseLocation) -> Result<(String, Vec<u8>)> {
        match location {
            ArchDatabaseLocation::Url(url) => {
                let response = self
                    .client
                    .get(url)
                    .send()
                    .await
                    .with_context(|| format!("requesting pacman sync database {url}"))?
                    .error_for_status()
                    .with_context(|| format!("pacman sync database returned an error: {url}"))?;
                let bytes = response
                    .bytes()
                    .await
                    .with_context(|| format!("reading pacman sync database {url}"))?;
                Ok((url.clone(), bytes.to_vec()))
            }
            ArchDatabaseLocation::Path(path) => {
                let bytes = tokio::fs::read(path)
                    .await
                    .with_context(|| format!("reading pacman sync database {}", path.display()))?;
                Ok((path.display().to_string(), bytes))
            }
        }
    }

    async fn load_package(
        &self,
        base: &ArchPackageBase,
        filename: &str,
    ) -> Result<(String, Vec<u8>)> {
        match base {
            ArchPackageBase::Url(base_url) => {
                let url = join_url(base_url, filename);
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("requesting pacman package {url}"))?
                    .error_for_status()
                    .with_context(|| format!("pacman package returned an error: {url}"))?;
                let bytes = response
                    .bytes()
                    .await
                    .with_context(|| format!("reading pacman package {url}"))?;
                Ok((url, bytes.to_vec()))
            }
            ArchPackageBase::Path(root) => {
                let path = root.join(filename);
                let bytes = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("reading pacman package {}", path.display()))?;
                Ok((path.display().to_string(), bytes))
            }
        }
    }
}

#[async_trait]
impl KernelConfigIndexer for ArchIndexer {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>> {
        let mut packages = Vec::new();
        let mut selected_package_count = 0usize;

        for feed in &self.config.feeds {
            let (database_source, database_bytes) = self.load_database(&feed.database).await?;
            let candidates = select_kernel_packages(
                &parse_sync_database(&database_bytes, &database_source)
                    .with_context(|| format!("parsing pacman sync database {database_source}"))?,
                &self.config.package_name_prefix,
                Some(feed.architecture.clone()),
                self.config.max_packages,
            );
            selected_package_count += candidates.len();

            for candidate in candidates {
                let (source, package_bytes) = self
                    .load_package(&feed.package_base, &candidate.filename)
                    .await?;
                let configs =
                    extract_kernel_configs_from_package(&package_bytes, &candidate.filename)
                        .with_context(|| format!("extracting kernel config from {source}"))?;

                for (config_path, config_text) in configs {
                    packages.push(KernelConfigPackage {
                        distribution: feed.distribution.clone(),
                        package_name: normalize_arch_kernel_package_name(&candidate.name),
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
                "Arch-family indexer did not find any packages matching prefix {:?}",
                self.config.package_name_prefix
            );
        }

        if packages.is_empty() {
            bail!(
                "Arch-family indexer selected {selected_package_count} package(s), but none contained a kernel config"
            );
        }

        Ok(packages)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchPackageCandidate {
    pub name: String,
    pub version: String,
    pub architecture: Architecture,
    pub filename: String,
}

pub fn parse_sync_database(bytes: &[u8], location_hint: &str) -> Result<Vec<ArchPackageCandidate>> {
    let tar_bytes = decode_tar_archive(bytes, location_hint)?;
    let mut archive = Archive::new(Cursor::new(tar_bytes));
    let mut packages = Vec::new();

    for entry in archive.entries().context("reading pacman sync database")? {
        let mut entry = entry.context("reading pacman sync database entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry.path().context("reading sync database entry path")?;
        if path.file_name().and_then(|name| name.to_str()) != Some("desc") {
            continue;
        }
        let path = path.display().to_string();

        let mut desc = String::new();
        entry
            .read_to_string(&mut desc)
            .with_context(|| format!("reading {path}"))?;
        if let Some(candidate) = parse_desc_file(&desc)? {
            packages.push(candidate);
        }
    }

    Ok(packages)
}

pub fn parse_desc_file(desc: &str) -> Result<Option<ArchPackageCandidate>> {
    let fields = parse_desc_fields(desc);
    let Some(name) = first_field(&fields, "NAME") else {
        return Ok(None);
    };
    let Some(version) = first_field(&fields, "VERSION") else {
        return Ok(None);
    };
    let Some(filename) = first_field(&fields, "FILENAME") else {
        return Ok(None);
    };
    let Some(architecture) = first_field(&fields, "ARCH") else {
        return Ok(None);
    };

    Ok(Some(ArchPackageCandidate {
        name: name.to_string(),
        version: version.to_string(),
        architecture: architecture.parse().map_err(anyhow::Error::msg)?,
        filename: filename.to_string(),
    }))
}

pub fn select_kernel_packages(
    packages: &[ArchPackageCandidate],
    package_name_prefix: &str,
    architecture: Option<Architecture>,
    max_packages: Option<usize>,
) -> Vec<ArchPackageCandidate> {
    let mut selected = packages
        .iter()
        .filter(|package| package.name.starts_with(package_name_prefix))
        .filter(|package| is_kernel_headers_package(&package.name))
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

pub fn normalize_arch_kernel_package_name(name: &str) -> String {
    name.strip_suffix("-headers")
        .or_else(|| name.strip_suffix("-devel"))
        .unwrap_or(name)
        .to_string()
}

pub fn extract_kernel_configs_from_package(
    package_bytes: &[u8],
    location_hint: &str,
) -> Result<Vec<(String, String)>> {
    let tar_bytes = decode_tar_archive(package_bytes, location_hint)?;
    let mut archive = Archive::new(Cursor::new(tar_bytes));
    let mut configs = Vec::new();

    for entry in archive
        .entries()
        .context("reading pacman package entries")?
    {
        let mut entry = entry.context("reading pacman package entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry.path().context("reading pacman package entry path")?;
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

fn parse_desc_fields(desc: &str) -> BTreeMap<String, Vec<String>> {
    let mut fields = BTreeMap::<String, Vec<String>>::new();
    let mut current_key: Option<String> = None;

    for line in desc.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('%') && line.ends_with('%') && line.len() > 2 {
            let key = line.trim_matches('%').to_string();
            fields.entry(key.clone()).or_default();
            current_key = Some(key);
            continue;
        }

        if let Some(key) = &current_key {
            fields
                .entry(key.clone())
                .or_default()
                .push(line.to_string());
        }
    }

    fields
}

fn first_field<'a>(fields: &'a BTreeMap<String, Vec<String>>, key: &str) -> Option<&'a str> {
    fields.get(key)?.first().map(String::as_str)
}

fn is_kernel_headers_package(name: &str) -> bool {
    name != "linux-api-headers" && (name.ends_with("-headers") || name.ends_with("-devel"))
}

fn decode_tar_archive(bytes: &[u8], location_hint: &str) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    if is_gzip(bytes, location_hint) {
        GzDecoder::new(Cursor::new(bytes))
            .read_to_end(&mut decoded)
            .context("decompressing gzip tar archive")?;
    } else if is_xz(bytes, location_hint) {
        XzDecoder::new(Cursor::new(bytes))
            .read_to_end(&mut decoded)
            .context("decompressing xz tar archive")?;
    } else if is_zstd(bytes, location_hint) {
        zstd::stream::read::Decoder::new(Cursor::new(bytes))
            .context("initializing zstd tar decoder")?
            .read_to_end(&mut decoded)
            .context("decompressing zstd tar archive")?;
    } else {
        decoded.extend_from_slice(bytes);
    }
    Ok(decoded)
}

fn is_gzip(bytes: &[u8], location_hint: &str) -> bool {
    bytes.starts_with(&[0x1f, 0x8b]) || location_hint.ends_with(".gz")
}

fn is_xz(bytes: &[u8], location_hint: &str) -> bool {
    bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) || location_hint.ends_with(".xz")
}

fn is_zstd(bytes: &[u8], location_hint: &str) -> bool {
    bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
        || location_hint.ends_with(".zst")
        || location_hint.ends_with(".zstd")
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

    (normalized.len() >= 5
        && (normalized[0] == "usr" && normalized[1] == "lib"
            || normalized[0] == "lib" && normalized[1] == "modules")
        && normalized.iter().any(|segment| segment == "modules")
        && normalized[normalized.len() - 2] == "build"
        && normalized[normalized.len() - 1] == ".config")
        || (normalized.len() >= 3
            && normalized[0] == "usr"
            && normalized[1] == "src"
            && normalized[normalized.len() - 1] == ".config")
}

fn arch_repo_root(
    mirror: &str,
    repository: &str,
    architecture: &Architecture,
    layout: ArchRepositoryLayout,
) -> String {
    let architecture = pacman_architecture_segment(architecture);
    match layout {
        ArchRepositoryLayout::RepoOsArch => {
            format!("{mirror}/{repository}/os/{architecture}")
        }
        ArchRepositoryLayout::RepoArch => {
            format!("{mirror}/{repository}/{architecture}")
        }
    }
}

fn default_layout(distribution: &Distribution) -> ArchRepositoryLayout {
    match distribution {
        Distribution::CachyOS => ArchRepositoryLayout::RepoArch,
        _ => ArchRepositoryLayout::RepoOsArch,
    }
}

fn pacman_architecture_segment(architecture: &Architecture) -> &str {
    match architecture {
        Architecture::Amd64 => "x86_64",
        Architecture::Arm64 => "aarch64",
        Architecture::Armhf => "armv7h",
        Architecture::I386 => "i686",
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
    fn parses_pacman_desc_file() {
        let candidate = parse_desc_file(
            r#"%FILENAME%
linux-6.12.1.arch1-1-x86_64.pkg.tar.zst

%NAME%
linux

%VERSION%
6.12.1.arch1-1

%ARCH%
x86_64
"#,
        )
        .expect("parse desc")
        .expect("candidate");

        assert_eq!(candidate.name, "linux");
        assert_eq!(candidate.version, "6.12.1.arch1-1");
        assert_eq!(candidate.architecture, Architecture::Amd64);
        assert_eq!(
            candidate.filename,
            "linux-6.12.1.arch1-1-x86_64.pkg.tar.zst"
        );
    }

    #[test]
    fn reads_gzip_sync_database() {
        let database = gzip_bytes(&tar_with_file(
            "linux-6.12.1.arch1-1/desc",
            br#"%FILENAME%
linux-6.12.1.arch1-1-x86_64.pkg.tar.zst

%NAME%
linux

%VERSION%
6.12.1.arch1-1

%ARCH%
x86_64
"#,
        ));

        let packages = parse_sync_database(&database, "core.db").expect("parse database");

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "linux");
    }

    #[test]
    fn selects_kernel_packages_without_companion_packages() {
        let packages = vec![
            candidate("linux", "6.12.1.arch1-1", Architecture::Amd64),
            candidate("linux-headers", "6.12.1.arch1-1", Architecture::Amd64),
            candidate("linux-api-headers", "6.19-1", Architecture::Amd64),
            candidate("linux-cachyos", "6.12.1-1", Architecture::Amd64),
            candidate("linux-cachyos-headers", "6.12.1-1", Architecture::Amd64),
            candidate("bash", "5.2-1", Architecture::Amd64),
        ];

        let selected = select_kernel_packages(&packages, "linux", Some(Architecture::Amd64), None);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].name, "linux-cachyos-headers");
        assert_eq!(selected[1].name, "linux-headers");
    }

    #[test]
    fn normalizes_header_package_name_to_kernel_package_name() {
        assert_eq!(normalize_arch_kernel_package_name("linux-headers"), "linux");
        assert_eq!(
            normalize_arch_kernel_package_name("linux-cachyos-headers"),
            "linux-cachyos"
        );
    }

    #[test]
    fn extracts_build_config_from_arch_package() {
        let package = zstd_bytes(&tar_with_file(
            "usr/lib/modules/6.12.1-arch1-1/build/.config",
            b"CONFIG_BPF=y\n",
        ));

        let configs =
            extract_kernel_configs_from_package(&package, "linux.pkg.tar.zst").expect("extract");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "usr/lib/modules/6.12.1-arch1-1/build/.config");
        assert!(configs[0].1.contains("CONFIG_BPF=y"));
    }

    #[test]
    fn builds_expected_repository_urls() {
        let arch = ArchIndexerConfig::from_mirror(
            Distribution::ArchLinux,
            "https://example.invalid",
            "core",
            [Architecture::Amd64],
        );
        let cachyos = ArchIndexerConfig::from_mirror(
            Distribution::CachyOS,
            "https://example.invalid/repo",
            "cachyos-v3",
            [Architecture::Amd64],
        );

        let arch_db = match &arch.feeds[0].database {
            ArchDatabaseLocation::Url(url) => url,
            ArchDatabaseLocation::Path(_) => panic!("expected URL database"),
        };
        let cachyos_db = match &cachyos.feeds[0].database {
            ArchDatabaseLocation::Url(url) => url,
            ArchDatabaseLocation::Path(_) => panic!("expected URL database"),
        };

        assert_eq!(arch_db, "https://example.invalid/core/os/x86_64/core.db");
        assert_eq!(
            cachyos_db,
            "https://example.invalid/repo/cachyos-v3/x86_64/cachyos-v3.db"
        );
    }

    fn candidate(name: &str, version: &str, architecture: Architecture) -> ArchPackageCandidate {
        ArchPackageCandidate {
            name: name.to_string(),
            version: version.to_string(),
            architecture,
            filename: format!("{name}-{version}.pkg.tar.zst"),
        }
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

    fn gzip_bytes(input: &[u8]) -> Vec<u8> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(input).expect("write gzip");
        gz.finish().expect("finish gzip")
    }

    fn zstd_bytes(input: &[u8]) -> Vec<u8> {
        zstd::encode_all(input, 0).expect("write zstd")
    }
}
