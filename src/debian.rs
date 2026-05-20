use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use flate2::read::GzDecoder;
use tar::Archive;
use xz2::read::XzDecoder;

use crate::indexer::{KernelConfigIndexer, KernelConfigPackage};

const DEFAULT_PACKAGE_PREFIX: &str = "linux-image-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageIndexLocation {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebianPackageBase {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebianPackageFeed {
    pub architecture: String,
    pub packages: PackageIndexLocation,
    pub deb_base: DebianPackageBase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebianIndexerConfig {
    pub feeds: Vec<DebianPackageFeed>,
    pub package_name_prefix: String,
    pub max_packages: Option<usize>,
}

impl DebianIndexerConfig {
    pub fn from_mirror(
        mirror: impl Into<String>,
        suite: impl AsRef<str>,
        component: impl AsRef<str>,
        architectures: impl IntoIterator<Item = String>,
    ) -> Self {
        let mirror = mirror.into().trim_end_matches('/').to_string();
        let suite = suite.as_ref();
        let component = component.as_ref();
        let feeds = architectures
            .into_iter()
            .map(|architecture| DebianPackageFeed {
                packages: PackageIndexLocation::Url(format!(
                    "{mirror}/dists/{suite}/{component}/binary-{architecture}/Packages.gz"
                )),
                deb_base: DebianPackageBase::Url(mirror.clone()),
                architecture,
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
pub struct DebianIndexer {
    config: DebianIndexerConfig,
    client: reqwest::Client,
}

impl DebianIndexer {
    pub fn new(config: DebianIndexerConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    async fn load_package_index(&self, location: &PackageIndexLocation) -> Result<Vec<u8>> {
        match location {
            PackageIndexLocation::Url(url) => {
                let response = self
                    .client
                    .get(url)
                    .send()
                    .await
                    .with_context(|| format!("requesting Debian package index {url}"))?
                    .error_for_status()
                    .with_context(|| format!("Debian package index returned an error: {url}"))?;
                response
                    .bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .with_context(|| format!("reading Debian package index {url}"))
            }
            PackageIndexLocation::Path(path) => tokio::fs::read(path)
                .await
                .with_context(|| format!("reading Debian package index {}", path.display())),
        }
    }

    async fn load_deb(
        &self,
        base: &DebianPackageBase,
        filename: &str,
    ) -> Result<(String, Vec<u8>)> {
        match base {
            DebianPackageBase::Url(base_url) => {
                let url = join_url(base_url, filename);
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("requesting Debian package {url}"))?
                    .error_for_status()
                    .with_context(|| format!("Debian package returned an error: {url}"))?;
                let bytes = response
                    .bytes()
                    .await
                    .with_context(|| format!("reading Debian package {url}"))?;
                Ok((url, bytes.to_vec()))
            }
            DebianPackageBase::Path(root) => {
                let path = root.join(filename);
                let bytes = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("reading Debian package {}", path.display()))?;
                Ok((path.display().to_string(), bytes))
            }
        }
    }
}

#[async_trait]
impl KernelConfigIndexer for DebianIndexer {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>> {
        let mut packages = Vec::new();

        for feed in &self.config.feeds {
            let package_index = self.load_package_index(&feed.packages).await?;
            let package_index_text = decode_package_index(&package_index, &feed.packages)?;
            let candidates = select_kernel_packages(
                &parse_packages_index(&package_index_text),
                &self.config.package_name_prefix,
                self.config.max_packages,
            );

            for candidate in candidates {
                let (source, deb_bytes) =
                    self.load_deb(&feed.deb_base, &candidate.filename).await?;
                let configs = extract_kernel_configs_from_deb(&deb_bytes)
                    .with_context(|| format!("extracting kernel config from {source}"))?;

                for (config_path, config_text) in configs {
                    packages.push(KernelConfigPackage {
                        distribution: "debian".to_string(),
                        package_name: candidate.name.clone(),
                        package_version: candidate.version.clone(),
                        architecture: feed.architecture.clone(),
                        source: Some(format!("{source}#{config_path}")),
                        config_text,
                    });
                }
            }
        }

        Ok(packages)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebianPackageCandidate {
    pub name: String,
    pub version: String,
    pub filename: String,
}

pub fn parse_packages_index(input: &str) -> Vec<BTreeMap<String, String>> {
    let mut stanzas = Vec::new();
    let mut current: BTreeMap<String, String> = BTreeMap::new();
    let mut current_key: Option<String> = None;

    for line in input.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                stanzas.push(std::mem::take(&mut current));
                current_key = None;
            }
            continue;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(key) = &current_key {
                current.entry(key.clone()).and_modify(|value| {
                    value.push('\n');
                    value.push_str(line.trim());
                });
            }
            continue;
        }

        if let Some((key, value)) = line.split_once(':') {
            let key = key.to_string();
            current.insert(key.clone(), value.trim_start().to_string());
            current_key = Some(key);
        }
    }

    if !current.is_empty() {
        stanzas.push(current);
    }

    stanzas
}

pub fn select_kernel_packages(
    stanzas: &[BTreeMap<String, String>],
    package_name_prefix: &str,
    max_packages: Option<usize>,
) -> Vec<DebianPackageCandidate> {
    let mut candidates = stanzas
        .iter()
        .filter_map(|stanza| {
            let name = stanza.get("Package")?;
            if !name.starts_with(package_name_prefix)
                || name.contains("-dbg")
                || name.contains("-dbgsym")
            {
                return None;
            }

            Some(DebianPackageCandidate {
                name: name.clone(),
                version: stanza.get("Version")?.clone(),
                filename: stanza.get("Filename")?.clone(),
            })
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        (&left.name, &left.version, &left.filename).cmp(&(
            &right.name,
            &right.version,
            &right.filename,
        ))
    });

    if let Some(max) = max_packages {
        candidates.truncate(max);
    }

    candidates
}

pub fn extract_kernel_configs_from_deb(deb_bytes: &[u8]) -> Result<Vec<(String, String)>> {
    let mut archive = ar::Archive::new(Cursor::new(deb_bytes));
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.context("reading ar member from Debian package")?;
        let identifier = String::from_utf8_lossy(entry.header().identifier())
            .trim()
            .trim_end_matches('/')
            .to_string();

        if identifier.starts_with("data.tar") {
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .with_context(|| format!("reading {identifier} from Debian package"))?;
            return extract_kernel_configs_from_data_tar(&identifier, &data);
        }
    }

    bail!("Debian package did not contain a data.tar member");
}

fn extract_kernel_configs_from_data_tar(
    identifier: &str,
    data: &[u8],
) -> Result<Vec<(String, String)>> {
    match identifier {
        "data.tar" => read_configs_from_tar(Cursor::new(data)),
        "data.tar.gz" => read_configs_from_tar(GzDecoder::new(Cursor::new(data))),
        "data.tar.xz" => read_configs_from_tar(XzDecoder::new(Cursor::new(data))),
        "data.tar.zst" | "data.tar.zstd" => {
            let decoder = zstd::stream::read::Decoder::new(Cursor::new(data))
                .context("initializing zstd decoder for data.tar")?;
            read_configs_from_tar(decoder)
        }
        other => Err(anyhow!("unsupported Debian data archive format {other}")),
    }
}

fn read_configs_from_tar(reader: impl Read) -> Result<Vec<(String, String)>> {
    let mut configs = Vec::new();
    let mut archive = Archive::new(reader);

    for entry in archive.entries().context("reading data.tar entries")? {
        let mut entry = entry.context("reading data.tar entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry.path().context("reading data.tar entry path")?;
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

    Ok(configs)
}

fn is_kernel_config_path(path: &Path) -> bool {
    let normalized = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();

    normalized.len() >= 2
        && normalized[normalized.len() - 2] == "boot"
        && normalized[normalized.len() - 1].starts_with("config-")
}

fn decode_package_index(bytes: &[u8], location: &PackageIndexLocation) -> Result<String> {
    let is_gzip = match location {
        PackageIndexLocation::Url(url) => url.ends_with(".gz"),
        PackageIndexLocation::Path(path) => {
            path.extension().is_some_and(|extension| extension == "gz")
        }
    };

    let mut decoded = String::new();
    if is_gzip {
        GzDecoder::new(Cursor::new(bytes))
            .read_to_string(&mut decoded)
            .context("decompressing Packages.gz")?;
    } else {
        decoded = String::from_utf8(bytes.to_vec()).context("decoding Packages index as UTF-8")?;
    }

    Ok(decoded)
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
    fn parses_debian_packages_stanzas() {
        let stanzas = parse_packages_index(
            r#"Package: linux-image-6.1.0-1-amd64
Version: 6.1.4-1
Architecture: amd64
Description: Linux kernel
 continuation line
Filename: pool/main/l/linux/linux-image.deb

Package: bash
Version: 5.2
Filename: pool/main/b/bash/bash.deb
"#,
        );

        assert_eq!(stanzas.len(), 2);
        assert_eq!(
            stanzas[0].get("Description").map(String::as_str),
            Some("Linux kernel\ncontinuation line")
        );
    }

    #[test]
    fn selects_kernel_image_packages_without_debug_symbols() {
        let stanzas = parse_packages_index(
            r#"Package: linux-image-6.1.0-1-amd64
Version: 6.1.4-1
Filename: pool/main/l/linux/linux-image.deb

Package: linux-image-6.1.0-1-amd64-dbgsym
Version: 6.1.4-1
Filename: pool/main/l/linux/linux-image-dbgsym.deb

Package: bash
Version: 5.2
Filename: pool/main/b/bash/bash.deb
"#,
        );

        let selected = select_kernel_packages(&stanzas, "linux-image-", None);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "linux-image-6.1.0-1-amd64");
    }

    #[test]
    fn extracts_boot_config_from_debian_package_data_archive() {
        let deb = minimal_deb_with_config("CONFIG_BPF=y\nCONFIG_EXT4_FS=m\n");

        let configs = extract_kernel_configs_from_deb(&deb).expect("extract configs");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "boot/config-6.1.0-1-amd64");
        assert!(configs[0].1.contains("CONFIG_BPF=y"));
    }

    fn minimal_deb_with_config(config: &str) -> Vec<u8> {
        let mut tarball = Vec::new();
        {
            let mut builder = Builder::new(&mut tarball);
            let mut header = Header::new_gnu();
            header.set_size(config.len() as u64);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "./boot/config-6.1.0-1-amd64",
                    config.as_bytes(),
                )
                .expect("append config");
            builder.finish().expect("finish tar");
        }

        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&tarball).expect("write gzip");
        let data_tar_gz = gz.finish().expect("finish gzip");

        let mut deb = b"!<arch>\n".to_vec();
        append_ar_member(&mut deb, "debian-binary", b"2.0\n");
        append_ar_member(&mut deb, "control.tar.gz", &[]);
        append_ar_member(&mut deb, "data.tar.gz", &data_tar_gz);
        deb
    }

    fn append_ar_member(ar: &mut Vec<u8>, name: &str, data: &[u8]) {
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8o}{:<10}`\n",
            name,
            0,
            0,
            0,
            0o100644,
            data.len()
        );
        assert_eq!(header.len(), 60);
        ar.extend_from_slice(header.as_bytes());
        ar.extend_from_slice(data);
        if !data.len().is_multiple_of(2) {
            ar.push(b'\n');
        }
    }
}
