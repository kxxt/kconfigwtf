use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use liblzma::read::XzDecoder;
use tar::Archive;

use crate::http::log_request_url;
use crate::ikconfig::extract_ikconfig_from_image;
use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage, normalize_apt_release_label};

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
    pub architecture: Architecture,
    pub packages: PackageIndexLocation,
    pub deb_base: DebianPackageBase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebianIndexerConfig {
    pub distribution: Distribution,
    pub release: String,
    pub feeds: Vec<DebianPackageFeed>,
    pub package_name_prefix: String,
    pub required_package_substrings: Vec<String>,
    pub excluded_package_substrings: Vec<String>,
    pub max_packages: Option<usize>,
}

impl DebianIndexerConfig {
    pub fn from_mirror(
        mirror: impl Into<String>,
        suite: impl AsRef<str>,
        component: impl AsRef<str>,
        architectures: impl IntoIterator<Item = Architecture>,
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
            distribution: Distribution::Debian,
            release: normalize_apt_release_label(suite),
            feeds,
            package_name_prefix: DEFAULT_PACKAGE_PREFIX.to_string(),
            required_package_substrings: Vec::new(),
            excluded_package_substrings: Vec::new(),
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
                log_request_url(url);
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
                log_request_url(&url);
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
        let mut selected_package_count = 0usize;

        for feed in &self.config.feeds {
            let package_index = self.load_package_index(&feed.packages).await?;
            let package_index_text = decode_package_index(&package_index, &feed.packages)?;
            let candidates = select_kernel_packages(
                &parse_packages_index(&package_index_text),
                &self.config.package_name_prefix,
                &self.config.required_package_substrings,
                &self.config.excluded_package_substrings,
                self.config.max_packages,
            );
            selected_package_count += candidates.len();

            for candidate in candidates {
                let (source, deb_bytes) =
                    self.load_deb(&feed.deb_base, &candidate.filename).await?;
                let configs = extract_kernel_configs_from_deb(&deb_bytes)
                    .with_context(|| format!("extracting kernel config from {source}"))?;

                for (config_path, config_text) in configs {
                    packages.push(KernelConfigPackage {
                        distribution: self.config.distribution.clone(),
                        release: self.config.release.clone(),
                        package_name: normalize_apt_kernel_package_name(
                            &candidate.name,
                            &self.config.package_name_prefix,
                            &feed.architecture,
                        ),
                        package_version: candidate.version.clone(),
                        architecture: feed.architecture.clone(),
                        source: Some(format!("{source}#{config_path}")),
                        config_text,
                    });
                }
            }
        }

        if selected_package_count == 0 {
            bail!(
                "APT indexer for {} did not find any packages matching prefix {:?}",
                self.config.distribution,
                self.config.package_name_prefix
            );
        }

        if packages.is_empty() {
            bail!(
                "APT indexer for {} selected {selected_package_count} package(s), but none contained a kernel config",
                self.config.distribution
            );
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
    required_substrings: &[String],
    excluded_substrings: &[String],
    max_packages: Option<usize>,
) -> Vec<DebianPackageCandidate> {
    let mut candidates = stanzas
        .iter()
        .filter_map(|stanza| {
            let name = stanza.get("Package")?;
            if !name.starts_with(package_name_prefix)
                || name.contains("-dbg")
                || name.contains("-dbgsym")
                || required_substrings
                    .iter()
                    .any(|substring| !name.contains(substring))
                || excluded_substrings
                    .iter()
                    .any(|substring| name.contains(substring))
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

pub fn normalize_debian_kernel_package_name(name: &str, architecture: &Architecture) -> String {
    normalize_apt_kernel_package_name(name, DEFAULT_PACKAGE_PREFIX, architecture)
}

pub fn normalize_apt_kernel_package_name(
    name: &str,
    package_name_prefix: &str,
    architecture: &Architecture,
) -> String {
    let Some(rest) = name.strip_prefix(package_name_prefix) else {
        return name.to_string();
    };
    let output_prefix = normalized_output_prefix(package_name_prefix);

    let mut segments = rest.split('-').collect::<Vec<_>>();
    if let Some(architecture_index) = segments
        .iter()
        .position(|segment| *segment == architecture.as_str())
    {
        segments[architecture_index] = "<ARCH>";
    }

    let Some(version_start) = segments
        .iter()
        .position(|segment| starts_with_digit(segment))
    else {
        return format!("{output_prefix}{}", segments.join("-"));
    };
    let version_prefix_len = kernel_version_prefix_len(&segments[version_start..]);

    let mut normalized = segments[..version_start].to_vec();
    normalized.push("<VERSION>");
    normalized.extend_from_slice(&segments[version_start + version_prefix_len..]);
    format!("{output_prefix}{}", normalized.join("-"))
}

fn normalized_output_prefix(package_name_prefix: &str) -> &str {
    match package_name_prefix {
        "linux-base-" => "linux-image-",
        "linux-modules-" => "linux-image-",
        other => other,
    }
}

fn kernel_version_prefix_len(segments: &[&str]) -> usize {
    if !segments
        .first()
        .is_some_and(|segment| starts_with_digit(segment))
    {
        return 0;
    }

    let mut len = 1;
    while len < segments.len()
        && segments[len]
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        len += 1;
    }
    len
}

fn starts_with_digit(segment: &str) -> bool {
    segment.starts_with(|character: char| character.is_ascii_digit())
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
        "data.tar.bz2" => read_configs_from_tar(BzDecoder::new(Cursor::new(data))),
        other => Err(anyhow!("unsupported Debian data archive format {other}")),
    }
}

fn read_configs_from_tar(reader: impl Read) -> Result<Vec<(String, String)>> {
    let mut configs = Vec::new();
    let mut image_candidates = Vec::new();
    let mut archive = Archive::new(reader);

    for entry in archive.entries().context("reading data.tar entries")? {
        let mut entry = entry.context("reading data.tar entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry.path().context("reading data.tar entry path")?;
        let path = path.display().to_string();
        if is_kernel_config_path(Path::new(&path)) {
            let mut config_text = String::new();
            entry
                .read_to_string(&mut config_text)
                .with_context(|| format!("reading kernel config {path}"))?;
            configs.push((path, config_text));
            continue;
        }

        if is_kernel_image_path(Path::new(&path)) {
            let mut image = Vec::new();
            entry
                .read_to_end(&mut image)
                .with_context(|| format!("reading kernel image {path}"))?;
            image_candidates.push((path, image));
        }
    }

    if configs.is_empty() {
        for (path, image) in image_candidates {
            let Ok(config_text) = extract_ikconfig_from_image(&image) else {
                continue;
            };
            configs.push((path, config_text));
        }
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

fn is_kernel_image_path(path: &Path) -> bool {
    let normalized = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();

    if normalized.len() < 2 || normalized[normalized.len() - 2] != "boot" {
        return false;
    }

    let filename = &normalized[normalized.len() - 1];
    filename.starts_with("vmlinuz-")
        || filename.starts_with("vmlinux-")
        || filename.starts_with("Image-")
        || filename.starts_with("bzImage-")
}

fn decode_package_index(bytes: &[u8], location: &PackageIndexLocation) -> Result<String> {
    let is_gzip = match location {
        PackageIndexLocation::Url(url) => url.ends_with(".gz"),
        PackageIndexLocation::Path(path) => {
            path.extension().is_some_and(|extension| extension == "gz")
        }
    };
    let is_xz = match location {
        PackageIndexLocation::Url(url) => url.ends_with(".xz"),
        PackageIndexLocation::Path(path) => {
            path.extension().is_some_and(|extension| extension == "xz")
        }
    };

    let mut decoded = String::new();
    if is_gzip {
        GzDecoder::new(Cursor::new(bytes))
            .read_to_string(&mut decoded)
            .context("decompressing Packages.gz")?;
    } else if is_xz {
        XzDecoder::new(Cursor::new(bytes))
            .read_to_string(&mut decoded)
            .context("decompressing Packages.xz")?;
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

        let selected = select_kernel_packages(&stanzas, "linux-image-", &[], &[], None);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "linux-image-6.1.0-1-amd64");
    }

    #[test]
    fn selects_packages_with_required_and_excluded_substrings() {
        let stanzas = parse_packages_index(
            r#"Package: proxmox-kernel-6.11.0-1-pve-signed
Version: 6.11.0-1
Filename: signed.deb

Package: proxmox-kernel-6.11.0-1-pve
Version: 6.11.0-1
Filename: unsigned.deb

Package: proxmox-kernel-6.11
Version: 6.11.0-1
Filename: meta.deb
"#,
        );
        let required = ["-pve".to_string()];
        let excluded = ["-signed".to_string()];

        let selected =
            select_kernel_packages(&stanzas, "proxmox-kernel-", &required, &excluded, None);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "proxmox-kernel-6.11.0-1-pve");
    }

    #[test]
    fn extracts_boot_config_from_debian_package_data_archive() {
        let deb = minimal_deb_with_config("CONFIG_BPF=y\nCONFIG_EXT4_FS=m\n");

        let configs = extract_kernel_configs_from_deb(&deb).expect("extract configs");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "boot/config-6.1.0-1-amd64");
        assert!(configs[0].1.contains("CONFIG_BPF=y"));
    }

    #[test]
    fn extracts_embedded_config_from_boot_kernel_image() {
        let image = fake_ikconfig_image("CONFIG_BPF=y\n# CONFIG_UNUSED is not set\n");
        let deb = minimal_deb_with_file("./boot/vmlinuz-6.18.27-aosc-main", &image);

        let configs = extract_kernel_configs_from_deb(&deb).expect("extract configs");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "boot/vmlinuz-6.18.27-aosc-main");
        assert!(configs[0].1.contains("CONFIG_BPF=y"));
    }

    #[test]
    fn normalizes_debian_kernel_package_name_for_storage_and_ui() {
        assert_eq!(
            normalize_debian_kernel_package_name(
                "linux-image-6.12.73+deb13-riscv64",
                &Architecture::Riscv64,
            ),
            "linux-image-<VERSION>-<ARCH>"
        );
        assert_eq!(
            normalize_debian_kernel_package_name(
                "linux-image-6.12.73+deb13-cloud-amd64",
                &Architecture::Amd64,
            ),
            "linux-image-<VERSION>-cloud-<ARCH>"
        );
        assert_eq!(
            normalize_debian_kernel_package_name(
                "linux-image-6.12.73+deb13-amd64-unsigned",
                &Architecture::Amd64,
            ),
            "linux-image-<VERSION>-<ARCH>-unsigned"
        );
        assert_eq!(
            normalize_debian_kernel_package_name("linux-image-6.1.0-1-amd64", &Architecture::Amd64),
            "linux-image-<VERSION>-<ARCH>"
        );
        assert_eq!(
            normalize_apt_kernel_package_name(
                "linux-base-6.19.14+kali-amd64",
                "linux-base-",
                &Architecture::Amd64,
            ),
            "linux-image-<VERSION>-<ARCH>"
        );
        assert_eq!(
            normalize_apt_kernel_package_name(
                "linux-modules-6.14.0-29-generic",
                "linux-modules-",
                &Architecture::Amd64,
            ),
            "linux-image-<VERSION>-generic"
        );
        assert_eq!(
            normalize_apt_kernel_package_name(
                "proxmox-kernel-6.11.0-1-pve",
                "proxmox-kernel-",
                &Architecture::Amd64,
            ),
            "proxmox-kernel-<VERSION>-pve"
        );
        assert_eq!(
            normalize_apt_kernel_package_name(
                "linux-kernel-rc-6.18.0",
                "linux-kernel-",
                &Architecture::Amd64,
            ),
            "linux-kernel-rc-<VERSION>"
        );
        assert_eq!(
            normalize_apt_kernel_package_name(
                "linux-kernel-vanillarc-7.0.0",
                "linux-kernel-",
                &Architecture::Amd64,
            ),
            "linux-kernel-vanillarc-<VERSION>"
        );
    }

    fn minimal_deb_with_config(config: &str) -> Vec<u8> {
        minimal_deb_with_file("./boot/config-6.1.0-1-amd64", config.as_bytes())
    }

    fn minimal_deb_with_file(path: &str, data: &[u8]) -> Vec<u8> {
        let mut tarball = Vec::new();
        {
            let mut builder = Builder::new(&mut tarball);
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, path, data)
                .expect("append data");
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

    fn fake_ikconfig_image(config: &str) -> Vec<u8> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(config.as_bytes()).expect("write gzip");
        let compressed = gz.finish().expect("finish gzip");

        let mut image = b"prefixIKCFG_ST".to_vec();
        image.extend_from_slice(&compressed);
        image.extend_from_slice(b"suffix");
        image
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
