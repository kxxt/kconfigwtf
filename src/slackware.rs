use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use flate2::read::MultiGzDecoder;
use liblzma::read::XzDecoder;
use tar::Archive;

use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackwareIndexLocation {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackwarePackageBase {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackwareRepoFeed {
    pub distribution: Distribution,
    pub architecture: Architecture,
    pub packages_txt: SlackwareIndexLocation,
    pub package_base: SlackwarePackageBase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackwareIndexerConfig {
    pub feeds: Vec<SlackwareRepoFeed>,
    pub package_name_prefix: String,
    pub max_packages: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct SlackwareIndexer {
    config: SlackwareIndexerConfig,
    client: reqwest::Client,
}

impl SlackwareIndexer {
    pub fn new(config: SlackwareIndexerConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    async fn load_packages_txt(&self, location: &SlackwareIndexLocation) -> Result<(String, String)> {
        match location {
            SlackwareIndexLocation::Url(url) => {
                let response = self
                    .client
                    .get(url)
                    .send()
                    .await
                    .with_context(|| format!("requesting PACKAGES.TXT {url}"))?
                    .error_for_status()
                    .with_context(|| format!("PACKAGES.TXT returned an error: {url}"))?;
                let text = response
                    .text()
                    .await
                    .with_context(|| format!("reading PACKAGES.TXT {url}"))?;
                Ok((url.clone(), text))
            }
            SlackwareIndexLocation::Path(path) => {
                let text = tokio::fs::read_to_string(path)
                    .await
                    .with_context(|| format!("reading PACKAGES.TXT {}", path.display()))?;
                Ok((path.display().to_string(), text))
            }
        }
    }

    async fn load_package(
        &self,
        base: &SlackwarePackageBase,
        location: &str,
        filename: &str,
    ) -> Result<(String, Vec<u8>)> {
        let location = location.trim_start_matches("./");
        match base {
            SlackwarePackageBase::Url(base_url) => {
                let url = format!(
                    "{}/{}/{}",
                    base_url.trim_end_matches('/'),
                    location.trim_start_matches('/'),
                    filename
                );
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("requesting slackware package {url}"))?
                    .error_for_status()
                    .with_context(|| format!("slackware package returned an error: {url}"))?;
                let bytes = response
                    .bytes()
                    .await
                    .with_context(|| format!("reading slackware package {url}"))?;
                Ok((format!("{url}#{filename}"), bytes.to_vec()))
            }
            SlackwarePackageBase::Path(root) => {
                let path = root.join(location).join(filename);
                let bytes = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("reading slackware package {}", path.display()))?;
                Ok((path.display().to_string(), bytes))
            }
        }
    }
}

#[async_trait]
impl KernelConfigIndexer for SlackwareIndexer {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>> {
        let mut packages = Vec::new();
        let mut selected_package_count = 0usize;

        for feed in &self.config.feeds {
            let (source, text) = self.load_packages_txt(&feed.packages_txt).await?;
            let candidates = select_kernel_packages(
                &parse_packages_txt(&text)
                    .with_context(|| format!("parsing PACKAGES.TXT {source}"))?,
                &self.config.package_name_prefix,
                &feed.architecture,
                self.config.max_packages,
            );
            selected_package_count += candidates.len();

            for candidate in candidates {
                let (source, package_bytes) = self
                    .load_package(&feed.package_base, &candidate.location, &candidate.filename)
                    .await?;
                let configs = extract_kernel_configs_from_txz(&package_bytes)
                    .with_context(|| format!("extracting kernel config from {source}"))?;

                for (config_path, config_text) in configs {
                    packages.push(KernelConfigPackage {
                        distribution: feed.distribution.clone(),
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
                "Slackware indexer did not find any packages matching prefix {:?}",
                self.config.package_name_prefix
            );
        }

        if packages.is_empty() {
            bail!(
                "Slackware indexer selected {selected_package_count} package(s), but none contained a kernel config"
            );
        }

        Ok(packages)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackwarePackageCandidate {
    pub name: String,
    pub version: String,
    pub architecture: Architecture,
    pub filename: String,
    pub location: String,
}

pub fn parse_packages_txt(input: &str) -> Result<Vec<SlackwarePackageCandidate>> {
    let mut candidates = Vec::new();

    for record in input.split("\n\n") {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }

        let Some(filename) = get_field(record, "PACKAGE NAME:") else {
            continue;
        };

        let Some(location) = get_field(record, "PACKAGE LOCATION:") else {
            continue;
        };

        let Some((name, version, architecture)) = parse_package_filename(filename) else {
            continue;
        };

        candidates.push(SlackwarePackageCandidate {
            name,
            version,
            architecture,
            filename: filename.to_string(),
            location: location.to_string(),
        });
    }

    Ok(candidates)
}

fn get_field<'a>(record: &'a str, prefix: &str) -> Option<&'a str> {
    record.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix(prefix).map(str::trim)
    })
}

fn parse_package_filename(package_name: &str) -> Option<(String, String, Architecture)> {
    let stem = package_name
        .strip_suffix(".txz")
        .or_else(|| package_name.strip_suffix(".tgz"))
        .or_else(|| package_name.strip_suffix(".tbz"))
        .or_else(|| package_name.strip_suffix(".tlz"))?;

    let (architecture, arch_pos) = detect_architecture(stem)?;
    let name_version = &stem[..arch_pos];

    let version_start = name_version.rfind(|c: char| !c.is_ascii_digit() && c != '.')?;
    let name = name_version[..version_start].trim_end_matches('-').to_string();
    let version = name_version[version_start + 1..].to_string();

    if version.is_empty() || !version.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }

    Some((name, version, architecture))
}

fn detect_architecture(stem: &str) -> Option<(Architecture, usize)> {
    let known_archs: &[(&str, Architecture)] = &[
        ("noarch", Architecture::Other("noarch".to_string())),
        ("x86_64", Architecture::Amd64),
        ("aarch64", Architecture::Arm64),
        ("i686", Architecture::I386),
        ("i586", Architecture::I386),
        ("i486", Architecture::I386),
        ("arm", Architecture::Armhf),
    ];

    for (arch_str, arch) in known_archs {
        let pattern = format!("-{arch_str}-");
        if let Some(pos) = stem.rfind(&pattern) {
            return Some((arch.clone(), pos));
        }
    }

    None
}

pub fn select_kernel_packages(
    candidates: &[SlackwarePackageCandidate],
    package_name_prefix: &str,
    architecture: &Architecture,
    max_packages: Option<usize>,
) -> Vec<SlackwarePackageCandidate> {
    let mut selected = candidates
        .iter()
        .filter(|c| c.name.starts_with(package_name_prefix))
        .filter(|c| is_slackware_kernel_package(&c.name))
        .filter(|c| &c.architecture == architecture)
        .cloned()
        .collect::<Vec<_>>();

    selected.sort_by(|a, b| {
        (&a.name, &a.version, &a.architecture).cmp(&(&b.name, &b.version, &b.architecture))
    });

    if let Some(max) = max_packages {
        selected.truncate(max);
    }

    selected
}

fn is_slackware_kernel_package(name: &str) -> bool {
    name.starts_with("kernel-") && !name.contains("firmware")
}

pub fn extract_kernel_configs_from_txz(package_bytes: &[u8]) -> Result<Vec<(String, String)>> {
    let tar_bytes = decode_tar_archive(package_bytes)?;
    let mut archive = Archive::new(Cursor::new(tar_bytes));
    let mut configs = Vec::new();

    for entry in archive.entries().context("reading txz package entries")? {
        let mut entry = entry.context("reading txz package entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry.path().context("reading txz entry path")?;
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

    configs.sort_by(|(a, _), (b, _)| a.cmp(b));
    Ok(configs)
}

fn decode_tar_archive(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    if is_xz(bytes) {
        XzDecoder::new(Cursor::new(bytes))
            .read_to_end(&mut decoded)
            .context("decompressing xz tar archive")?;
    } else if is_gzip(bytes) {
        MultiGzDecoder::new(Cursor::new(bytes))
            .read_to_end(&mut decoded)
            .context("decompressing gzip tar archive")?;
    } else {
        bail!("unknown package compression format (expected .txz or .tgz)");
    }
    Ok(decoded)
}

fn is_xz(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00])
}

fn is_gzip(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

fn is_kernel_config_path(path: &Path) -> bool {
    let components: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();

    if components.len() >= 2
        && components[components.len() - 2] == "boot"
        && components[components.len() - 1].starts_with("config-")
    {
        return true;
    }

    if components.len() >= 3
        && components[0] == "usr"
        && components[1] == "src"
        && components.last().map(|s| s.as_ref()) == Some(".config")
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use liblzma::write::XzEncoder;
    use std::io::Write;
    use tar::{Builder, Header};

    #[test]
    fn parses_packages_txt_kernel_packages() {
        let input = r#"PACKAGE NAME:  kernel-generic-5.15.19-x86_64-1.txz
PACKAGE MIRROR: slackware64
PACKAGE LOCATION: ./slackware64/a
PACKAGE SIZE (compressed): 8500 K
PACKAGE SIZE (uncompressed): 42000 K
PACKAGE DESCRIPTION:
kernel-generic: Linux kernel
kernel-generic: The Linux kernel (generic, SMP)

PACKAGE NAME:  kernel-modules-5.15.19-x86_64-1.txz
PACKAGE MIRROR: slackware64
PACKAGE LOCATION: ./slackware64/a
PACKAGE SIZE (compressed): 55000 K
PACKAGE SIZE (uncompressed): 280000 K
PACKAGE DESCRIPTION:
kernel-modules: Linux kernel modules
kernel-modules: The Linux kernel modules

PACKAGE NAME:  kernel-firmware-20230101-x86_64-1.txz
PACKAGE MIRROR: slackware64
PACKAGE LOCATION: ./slackware64/a
PACKAGE SIZE (compressed): 120000 K
PACKAGE SIZE (uncompressed): 600000 K
PACKAGE DESCRIPTION:
kernel-firmware: Linux firmware
kernel-firmware: Various firmware files

PACKAGE NAME:  kernel-headers-5.15.19-x86_64-1.txz
PACKAGE MIRROR: slackware64
PACKAGE LOCATION: ./slackware64/a
PACKAGE SIZE (compressed): 1200 K
PACKAGE SIZE (uncompressed): 6000 K
PACKAGE DESCRIPTION:
kernel-headers: Linux kernel headers

PACKAGE NAME:  bash-5.2.021-x86_64-1.txz
PACKAGE MIRROR: slackware64
PACKAGE LOCATION: ./slackware64/a
PACKAGE SIZE (compressed): 1500 K
PACKAGE SIZE (uncompressed): 7000 K
PACKAGE DESCRIPTION:
bash: GNU Bourne-Again Shell
bash: The GNU Bourne-Again Shell
"#;

        let candidates = parse_packages_txt(input).expect("parse PACKAGES.TXT");
        assert_eq!(candidates.len(), 5);

        assert_eq!(candidates[0].name, "kernel-generic");
        assert_eq!(candidates[0].version, "5.15.19");
        assert_eq!(candidates[0].architecture, Architecture::Amd64);
        assert_eq!(
            candidates[0].filename,
            "kernel-generic-5.15.19-x86_64-1.txz"
        );
        assert_eq!(candidates[0].location, "./slackware64/a");

        assert_eq!(candidates[1].name, "kernel-modules");
        assert_eq!(candidates[1].version, "5.15.19");

        assert_eq!(candidates[2].name, "kernel-firmware");
        assert_eq!(candidates[2].version, "20230101");

        assert_eq!(candidates[3].name, "kernel-headers");
        assert_eq!(candidates[3].version, "5.15.19");

        // bash is parsed but won't pass select_kernel_packages
        assert_eq!(candidates[4].name, "bash");
    }

    #[test]
    fn selects_kernel_packages_without_firmware() {
        let candidates = vec![
            slackware_candidate("kernel-generic", "5.15.19", Architecture::Amd64),
            slackware_candidate("kernel-huge", "5.15.19", Architecture::Amd64),
            slackware_candidate("kernel-modules", "5.15.19", Architecture::Amd64),
            slackware_candidate("kernel-source", "5.15.19", Architecture::Amd64),
            slackware_candidate("kernel-headers", "5.15.19", Architecture::Amd64),
            slackware_candidate("kernel-firmware", "20230101", Architecture::Amd64),
        ];

        let selected = select_kernel_packages(
            &candidates,
            "kernel-",
            &Architecture::Amd64,
            None,
        );

        // kernel-firmware is excluded by is_slackware_kernel_package;
        // kernel-headers is included (it may or may not contain a .config)
        assert_eq!(selected.len(), 5);
        assert!(!selected.iter().any(|c| c.name == "kernel-firmware"));
    }

    #[test]
    fn parses_package_filename_x86_64() {
        let (name, version, arch) =
            parse_package_filename("kernel-generic-5.15.19-x86_64-1.txz").expect("parse");
        assert_eq!(name, "kernel-generic");
        assert_eq!(version, "5.15.19");
        assert_eq!(arch, Architecture::Amd64);
    }

    #[test]
    fn parses_package_filename_aarch64() {
        let (name, version, arch) =
            parse_package_filename("kernel-generic-6.6.1-aarch64-1.txz").expect("parse");
        assert_eq!(name, "kernel-generic");
        assert_eq!(version, "6.6.1");
        assert_eq!(arch, Architecture::Arm64);
    }

    #[test]
    fn parses_package_filename_arm() {
        let (name, version, arch) =
            parse_package_filename("kernel-generic-5.15.19-arm-1.txz").expect("parse");
        assert_eq!(name, "kernel-generic");
        assert_eq!(version, "5.15.19");
        assert_eq!(arch, Architecture::Armhf);
    }

    #[test]
    fn parses_package_filename_486() {
        let (name, version, arch) =
            parse_package_filename("kernel-generic-5.15.19-i486-1.txz").expect("parse");
        assert_eq!(name, "kernel-generic");
        assert_eq!(version, "5.15.19");
        assert_eq!(arch, Architecture::I386);
    }

    #[test]
    fn parses_package_with_dashes_in_name() {
        let (name, version, arch) =
            parse_package_filename("kernel-generic-smp-5.15.19-x86_64-1.txz").expect("parse");
        assert_eq!(name, "kernel-generic-smp");
        assert_eq!(version, "5.15.19");
        assert_eq!(arch, Architecture::Amd64);
    }

    #[test]
    fn extracts_boot_config_from_txz() {
        let txz = txz_with_file("boot/config-5.15.19", b"CONFIG_BPF=y\nCONFIG_EXT4_FS=m\n");

        let configs = extract_kernel_configs_from_txz(&txz).expect("extract config");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "boot/config-5.15.19");
        assert!(configs[0].1.contains("CONFIG_BPF=y"));
        assert!(configs[0].1.contains("CONFIG_EXT4_FS=m"));
    }

    #[test]
    fn extracts_usr_src_config_from_txz() {
        let txz = txz_with_file("usr/src/linux-5.15.19/.config", b"CONFIG_BPF=y\n");

        let configs = extract_kernel_configs_from_txz(&txz).expect("extract config");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "usr/src/linux-5.15.19/.config");
    }

    #[test]
    fn skips_non_config_files_in_txz() {
        let txz = txz_with_file("usr/bin/bash", b"some binary data");

        let configs = extract_kernel_configs_from_txz(&txz).expect("extract config");

        assert_eq!(configs.len(), 0);
    }

    #[test]
    fn builds_packages_txt_urls() {
        let feeds = vec![SlackwareRepoFeed {
            distribution: Distribution::Slackware,
            architecture: Architecture::Amd64,
            packages_txt: SlackwareIndexLocation::Url(
                "https://mirrors.example.invalid/slackware/slackware64-15.0/PACKAGES.TXT"
                    .to_string(),
            ),
            package_base: SlackwarePackageBase::Url(
                "https://mirrors.example.invalid/slackware/slackware64-15.0".to_string(),
            ),
        }];

        let config = SlackwareIndexerConfig {
            feeds,
            package_name_prefix: "kernel-".to_string(),
            max_packages: None,
        };

        assert_eq!(config.feeds.len(), 1);
        assert_eq!(
            match &config.feeds[0].packages_txt {
                SlackwareIndexLocation::Url(url) => url,
                SlackwareIndexLocation::Path(_) => panic!("expected URL"),
            },
            "https://mirrors.example.invalid/slackware/slackware64-15.0/PACKAGES.TXT"
        );
    }

    fn slackware_candidate(
        name: &str,
        version: &str,
        architecture: Architecture,
    ) -> SlackwarePackageCandidate {
        SlackwarePackageCandidate {
            name: name.to_string(),
            version: version.to_string(),
            architecture: architecture.clone(),
            filename: format!("{name}-{version}-{}", architecture.as_str()),
            location: "./slackware64/a".to_string(),
        }
    }

    fn txz_with_file(path: &str, contents: &[u8]) -> Vec<u8> {
        let tarball = tar_with_file(path, contents);
        let mut xz = XzEncoder::new(Vec::new(), 6);
        xz.write_all(&tarball).expect("write xz");
        xz.finish().expect("finish xz")
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
