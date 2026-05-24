use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use tar::Archive;
use zstd::stream::read::Decoder as ZstdDecoder;

use crate::ikconfig::looks_like_kernel_config;
use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage};

pub const DEFAULT_TARGETS_URL: &str = "https://downloads.openwrt.org/snapshots/targets";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenWrtTargetsLocation {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWrtIndexerConfig {
    pub targets: OpenWrtTargetsLocation,
    pub selected_targets: Vec<String>,
    pub max_targets: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct OpenWrtIndexer {
    config: OpenWrtIndexerConfig,
    client: reqwest::Client,
}

impl OpenWrtIndexer {
    pub fn new(config: OpenWrtIndexerConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    async fn discover_target_pairs(&self) -> Result<Vec<String>> {
        let mut targets = if self.config.selected_targets.is_empty() {
            self.discover_all_target_pairs().await?
        } else {
            self.resolve_selected_target_pairs().await?
        };

        targets.sort();
        targets.dedup();
        if let Some(max) = self.config.max_targets {
            targets.truncate(max);
        }
        Ok(targets)
    }

    async fn discover_all_target_pairs(&self) -> Result<Vec<String>> {
        let mut pairs = Vec::new();
        for target in self.list_directory_entries(None).await? {
            for subtarget in self.list_directory_entries(Some(&target)).await? {
                pairs.push(format!("{target}/{subtarget}"));
            }
        }
        Ok(pairs)
    }

    async fn resolve_selected_target_pairs(&self) -> Result<Vec<String>> {
        let mut pairs = Vec::new();
        for selected in &self.config.selected_targets {
            let selected = selected.trim().trim_matches('/');
            if selected.is_empty() {
                continue;
            }

            if let Some((target, subtarget)) = selected.split_once('/') {
                if target.contains('/') || subtarget.contains('/') {
                    bail!("invalid OpenWrt target path {selected:?}");
                }
                pairs.push(format!("{target}/{subtarget}"));
                continue;
            }

            for subtarget in self.list_directory_entries(Some(selected)).await? {
                pairs.push(format!("{selected}/{subtarget}"));
            }
        }
        Ok(pairs)
    }

    async fn list_directory_entries(&self, path: Option<&str>) -> Result<Vec<String>> {
        match &self.config.targets {
            OpenWrtTargetsLocation::Url(root) => {
                let url = target_listing_url(root, path);
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("requesting OpenWrt target listing {url}"))?
                    .error_for_status()
                    .with_context(|| format!("OpenWrt target listing returned an error: {url}"))?;
                let html = response
                    .text()
                    .await
                    .with_context(|| format!("reading OpenWrt target listing {url}"))?;
                Ok(parse_directory_entries(&html))
            }
            OpenWrtTargetsLocation::Path(root) => {
                let directory = path.map_or_else(|| root.clone(), |path| root.join(path));
                read_local_directory_entries(&directory)
            }
        }
    }

    async fn load_target_artifact(&self, target: &str, filename: &str) -> Result<(String, String)> {
        match &self.config.targets {
            OpenWrtTargetsLocation::Url(root) => {
                let url = target_artifact_url(root, target, filename);
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("requesting OpenWrt artifact {url}"))?
                    .error_for_status()
                    .with_context(|| format!("OpenWrt artifact returned an error: {url}"))?;
                let text = response
                    .text()
                    .await
                    .with_context(|| format!("reading OpenWrt artifact {url}"))?;
                Ok((url, text))
            }
            OpenWrtTargetsLocation::Path(root) => {
                let path = root.join(target).join(filename);
                let text = tokio::fs::read_to_string(&path)
                    .await
                    .with_context(|| format!("reading OpenWrt artifact {}", path.display()))?;
                Ok((path.display().to_string(), text))
            }
        }
    }

    async fn fetch_kernel_config(
        &self,
        target: &str,
        profiles: &OpenWrtProfiles,
    ) -> Result<(String, String)> {
        match &self.config.targets {
            OpenWrtTargetsLocation::Url(root) => {
                self.fetch_kernel_config_from_imagebuilder(root, target, profiles)
                    .await
            }
            OpenWrtTargetsLocation::Path(root) => {
                let dir = root.join(target);
                let candidates = ["kernel.config", ".config", "config"];
                for name in &candidates {
                    let path = dir.join(name);
                    if path.exists() {
                        let text = tokio::fs::read_to_string(&path)
                            .await
                            .with_context(|| format!("reading {}", path.display()))?;
                        if looks_like_kernel_config(&text) {
                            return Ok((path.display().to_string(), text));
                        }
                    }
                }
                bail!(
                    "no kernel config found in {} (tried {})",
                    dir.display(),
                    candidates.join(", ")
                );
            }
        }
    }

    async fn fetch_kernel_config_from_imagebuilder(
        &self,
        root: &str,
        target: &str,
        profiles: &OpenWrtProfiles,
    ) -> Result<(String, String)> {
        let normalized = normalized_target_name(&profiles.target);
        let owrt_version = profiles.version_number.as_deref().unwrap();
        let mut last_error = None;
        for suffix in ["zst", "xz"] {
            let ib_filename = format!(
                "openwrt-imagebuilder-{owrt_version}-{normalized}.Linux-x86_64.tar.{suffix}"
            );
            let url = target_artifact_url(root, target, &ib_filename);

            let response = match self
                .client
                .get(&url)
                .send()
                .await
                .with_context(|| format!("downloading imagebuilder {url}"))?
                .error_for_status()
                .with_context(|| format!("imagebuilder returned an error: {url}"))
            {
                Ok(r) => r,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let data = response
                .bytes()
                .await
                .with_context(|| format!("reading imagebuilder {url}"))?;

            let config_text =
                extract_kernel_config_from_imagebuilder(&data, suffix).with_context(|| {
                    format!("extracting kernel config from imagebuilder for {target}")
                })?;

            return Ok((url, config_text));
        }
        Err(last_error.unwrap())
    }
}

#[async_trait]
impl KernelConfigIndexer for OpenWrtIndexer {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>> {
        let targets = self.discover_target_pairs().await?;
        if targets.is_empty() {
            bail!("OpenWrt indexer did not discover any targets");
        }

        let mut packages = Vec::new();
        for target in targets {
            let (_profiles_source, profiles_text) =
                self.load_target_artifact(&target, "profiles.json").await?;
            let profiles = parse_profiles_json(&profiles_text)
                .with_context(|| format!("parsing OpenWrt profiles metadata for {target}"))?;

            let (config_source, config_text) = self.fetch_kernel_config(&target, &profiles).await?;
            if !looks_like_kernel_config(&config_text) {
                bail!(
                    "OpenWrt target {target} did not provide a valid kernel config in {config_source}"
                );
            }

            packages.push(KernelConfigPackage {
                distribution: Distribution::OpenWrt,
                package_name: normalized_target_name(&profiles.target),
                package_version: target_build_version(&profiles),
                architecture: architecture_from_arch_packages(&profiles.arch_packages),
                source: Some(config_source),
                config_text,
            });
        }

        Ok(packages)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OpenWrtProfiles {
    pub arch_packages: String,
    pub linux_kernel: OpenWrtLinuxKernel,
    pub target: String,
    pub version_code: Option<String>,
    pub version_number: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OpenWrtLinuxKernel {
    pub version: String,
}

pub fn parse_profiles_json(input: &str) -> Result<OpenWrtProfiles> {
    serde_json::from_str(input).context("parsing OpenWrt profiles.json")
}

pub fn extract_kernel_config_from_imagebuilder(data: &[u8], compression: &str) -> Result<String> {
    let decoder: &mut dyn Read = match compression {
        "xz" => &mut liblzma::read::XzDecoder::new(Cursor::new(data)) as _,
        "zst" | "zstd" => &mut ZstdDecoder::new(Cursor::new(data))
            .context("initializing zstd decoder for imagebuilder")? as _,
        other => todo!("unsupported compression type {other}"),
    };
    let mut archive = Archive::new(decoder);

    for entry_result in archive
        .entries()
        .context("reading imagebuilder tar entries")?
    {
        let mut entry = entry_result.context("reading imagebuilder tar entry")?;
        let path = entry.path().ok().map(|p| p.to_string_lossy().to_string());

        let Some(path) = path else {
            continue;
        };

        if !path.contains("/build_dir/") || !path.ends_with("/.config") {
            continue;
        }

        let mut content = String::new();
        entry
            .read_to_string(&mut content)
            .with_context(|| format!("reading kernel config from {path}"))?;

        if looks_like_kernel_config(&content) {
            return Ok(content);
        }
    }

    bail!("kernel config not found in imagebuilder tarball")
}

pub fn parse_directory_entries(input: &str) -> Vec<String> {
    let mut entries = Vec::new();
    for segment in input.split("href=\"").skip(1) {
        let Some((href, _)) = segment.split_once('"') else {
            continue;
        };
        let Some(entry) = normalize_directory_href(href) else {
            continue;
        };
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    entries
}

pub fn normalized_target_name(target: &str) -> String {
    target.trim_matches('/').replace('/', "-")
}

pub fn target_build_version(profiles: &OpenWrtProfiles) -> String {
    let build = match profiles.version_number.as_deref() {
        Some("SNAPSHOT") => profiles
            .version_code
            .as_deref()
            .map(|version_code| format!("SNAPSHOT-{version_code}"))
            .unwrap_or_else(|| "SNAPSHOT".to_string()),
        Some(version) if !version.trim().is_empty() => version.to_string(),
        _ => profiles
            .version_code
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
    };

    if profiles.linux_kernel.version.trim().is_empty() {
        build
    } else {
        format!("{build}-kernel-{}", profiles.linux_kernel.version)
    }
}

pub fn architecture_from_arch_packages(value: &str) -> Architecture {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Architecture::Other(value.to_string());
    }

    if normalized.starts_with("x86_64") {
        return Architecture::Amd64;
    }
    if normalized.starts_with("aarch64") {
        return Architecture::Arm64;
    }
    if normalized.starts_with("arm_") || normalized.starts_with("armv7") {
        return Architecture::Armhf;
    }
    if normalized.starts_with("i386") || normalized.starts_with("i486") {
        return Architecture::I386;
    }
    if normalized.starts_with("powerpc64le") {
        return Architecture::Ppc64el;
    }
    if normalized.starts_with("riscv64") {
        return Architecture::Riscv64;
    }
    if normalized.starts_with("s390x") {
        return Architecture::S390x;
    }

    Architecture::Other(value.to_string())
}

fn read_local_directory_entries(path: &Path) -> Result<Vec<String>> {
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("reading OpenWrt directory {}", path.display()))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let is_dir = entry.file_type().ok()?.is_dir();
            if !is_dir {
                return None;
            }
            entry.file_name().into_string().ok()
        })
        .collect::<Vec<_>>();
    entries.sort();
    Ok(entries)
}

fn normalize_directory_href(href: &str) -> Option<String> {
    let href = href.trim();
    if !href.ends_with('/') || href.starts_with('?') {
        return None;
    }

    let entry = href.trim_end_matches('/');
    if entry.is_empty()
        || entry == "."
        || entry == ".."
        || entry.contains('/')
        || entry.contains('\\')
    {
        return None;
    }

    Some(entry.to_string())
}

fn target_listing_url(root: &str, path: Option<&str>) -> String {
    match path {
        Some(path) => format!("{}/{}/", root.trim_end_matches('/'), path.trim_matches('/')),
        None => format!("{}/", root.trim_end_matches('/')),
    }
}

fn target_artifact_url(root: &str, target: &str, filename: &str) -> String {
    format!(
        "{}/{}/{}",
        root.trim_end_matches('/'),
        target.trim_matches('/'),
        filename
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_directory_entries_from_openwrt_index() {
        let entries = parse_directory_entries(
            r#"
<a href="../">Parent directory/</a>
<a href="x86/">x86/</a>
<a href="ramips/">ramips/</a>
<a href="?C=N;O=D">Sort by name</a>
"#,
        );

        assert_eq!(entries, vec!["x86", "ramips"]);
    }

    #[test]
    fn builds_snapshot_target_version() {
        let version = target_build_version(&OpenWrtProfiles {
            arch_packages: "x86_64".to_string(),
            linux_kernel: OpenWrtLinuxKernel {
                version: "6.18.31".to_string(),
            },
            target: "x86/64".to_string(),
            version_code: Some("r34569-49b5093679".to_string()),
            version_number: Some("SNAPSHOT".to_string()),
        });

        assert_eq!(version, "SNAPSHOT-r34569-49b5093679-kernel-6.18.31");
    }

    #[test]
    fn maps_openwrt_arch_packages_to_common_architecture_names() {
        assert_eq!(
            architecture_from_arch_packages("x86_64"),
            Architecture::Amd64
        );
        assert_eq!(
            architecture_from_arch_packages("aarch64_cortex-a53"),
            Architecture::Arm64
        );
        assert_eq!(
            architecture_from_arch_packages("arm_cortex-a7_neon-vfpv4"),
            Architecture::Armhf
        );
        assert_eq!(
            architecture_from_arch_packages("mipsel_24kc"),
            Architecture::Other("mipsel_24kc".to_string())
        );
    }

    #[test]
    fn extracts_kernel_config_from_imagebuilder() {
        let config_content =
            "# Linux/x86 6.12.71 Kernel Configuration\nCONFIG_TEST=y\n# CONFIG_FOO is not set\n";

        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            let mut header = {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Regular);
                h.set_size(config_content.len() as u64);
                h.set_mode(0o644);
                h
            };
            builder
                .append_data(
                    &mut header,
                    "openwrt-imagebuilder-x86-64.Linux-x86_64/build_dir/target-x86_64_musl/linux-x86_64/linux-6.12.71/.config",
                    config_content.as_bytes(),
                )
                .expect("append kernel config to tar");

            builder.finish().expect("finish tar");
        }

        let compressed = zstd::encode_all(&tar_buf[..], 0).expect("compress tar");

        let result =
            extract_kernel_config_from_imagebuilder(&compressed, "zst").expect("extract config");
        assert!(result.contains("CONFIG_TEST=y"));
        assert!(result.contains("Kernel Configuration"));
    }
}
