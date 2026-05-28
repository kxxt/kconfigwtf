use std::io::{BufReader, Cursor, Read};
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use flate2::read::GzDecoder;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::http::log_request_url;
use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage, normalize_rpm_release_label};

const DEFAULT_PACKAGE_NAME: &str = "kernel-core";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FedoraMetadataLocation {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FedoraPackageBase {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FedoraRepoFeed {
    pub architecture: Architecture,
    pub package_architecture: Option<Architecture>,
    pub repomd: FedoraMetadataLocation,
    pub package_base: FedoraPackageBase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FedoraIndexerConfig {
    pub distribution: Distribution,
    pub release: String,
    pub feeds: Vec<FedoraRepoFeed>,
    pub package_name: String,
    pub package_names: Vec<String>,
    pub max_packages: Option<usize>,
}

impl FedoraIndexerConfig {
    pub fn from_mirror(
        mirror: impl Into<String>,
        release: impl AsRef<str>,
        architectures: impl IntoIterator<Item = Architecture>,
    ) -> Self {
        let mirror = mirror.into().trim_end_matches('/').to_string();
        let release = release.as_ref();
        let feeds = architectures
            .into_iter()
            .map(|architecture| {
                let repo_root = fedora_repo_root(&mirror, release, &architecture);
                FedoraRepoFeed {
                    architecture,
                    package_architecture: None,
                    repomd: FedoraMetadataLocation::Url(format!("{repo_root}/repodata/repomd.xml")),
                    package_base: FedoraPackageBase::Url(repo_root),
                }
            })
            .collect();

        Self {
            distribution: Distribution::Fedora,
            release: normalize_rpm_release_label(&Distribution::Fedora, release),
            feeds,
            package_name: DEFAULT_PACKAGE_NAME.to_string(),
            package_names: vec![DEFAULT_PACKAGE_NAME.to_string()],
            max_packages: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FedoraIndexer {
    config: FedoraIndexerConfig,
    client: reqwest::Client,
}

impl FedoraIndexer {
    pub fn new(config: FedoraIndexerConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    async fn load_metadata(&self, location: &FedoraMetadataLocation) -> Result<Vec<u8>> {
        match location {
            FedoraMetadataLocation::Url(url) => {
                log_request_url(url);
                let response = self
                    .client
                    .get(url)
                    .send()
                    .await
                    .with_context(|| format!("requesting Fedora metadata {url}"))?
                    .error_for_status()
                    .with_context(|| format!("Fedora metadata returned an error: {url}"))?;
                response
                    .bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .with_context(|| format!("reading Fedora metadata {url}"))
            }
            FedoraMetadataLocation::Path(path) => tokio::fs::read(path)
                .await
                .with_context(|| format!("reading Fedora metadata {}", path.display())),
        }
    }

    async fn load_repo_file(
        &self,
        base: &FedoraPackageBase,
        href: &str,
    ) -> Result<(String, Vec<u8>)> {
        match base {
            FedoraPackageBase::Url(base_url) => {
                let url = join_url(base_url, href);
                log_request_url(&url);
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("requesting Fedora repo file {url}"))?
                    .error_for_status()
                    .with_context(|| format!("Fedora repo file returned an error: {url}"))?;
                let bytes = response
                    .bytes()
                    .await
                    .with_context(|| format!("reading Fedora repo file {url}"))?;
                Ok((url, bytes.to_vec()))
            }
            FedoraPackageBase::Path(root) => {
                let path = root.join(href);
                let bytes = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("reading Fedora repo file {}", path.display()))?;
                Ok((path.display().to_string(), bytes))
            }
        }
    }
}

#[async_trait]
impl KernelConfigIndexer for FedoraIndexer {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>> {
        let mut packages = Vec::new();
        let mut selected_package_count = 0usize;

        for feed in &self.config.feeds {
            let repomd = self.load_metadata(&feed.repomd).await?;
            let repomd_text = String::from_utf8(repomd).context("decoding Fedora repomd.xml")?;
            let primary_href = parse_primary_href(&repomd_text)?;
            let (primary_source, primary_bytes) = self
                .load_repo_file(&feed.package_base, &primary_href)
                .await?;
            let primary_text = decode_repo_metadata(&primary_bytes, &primary_href)
                .with_context(|| format!("decoding Fedora primary metadata {primary_source}"))?;
            let candidates = select_kernel_packages(
                &parse_primary_metadata(&primary_text)?,
                &self.config.package_names,
                Some(
                    feed.package_architecture
                        .clone()
                        .unwrap_or_else(|| feed.architecture.clone()),
                ),
                self.config.max_packages,
            );
            selected_package_count += candidates.len();

            for candidate in candidates {
                let (source, rpm_bytes) = self
                    .load_repo_file(&feed.package_base, &candidate.location_href)
                    .await?;
                let configs = extract_kernel_configs_from_rpm(&rpm_bytes)
                    .with_context(|| format!("extracting kernel config from {source}"))?;

                for (config_path, config_text) in configs {
                    packages.push(KernelConfigPackage {
                        distribution: self.config.distribution.clone(),
                        release: self.config.release.clone(),
                        package_name: candidate.name.clone(),
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
                "RPM indexer did not find any packages named {:?}",
                self.config.package_name
            );
        }

        if packages.is_empty() {
            bail!(
                "RPM indexer selected {selected_package_count} package(s), but none contained a kernel config"
            );
        }

        Ok(packages)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FedoraPackageCandidate {
    pub name: String,
    pub version: String,
    pub architecture: Architecture,
    pub location_href: String,
}

pub fn parse_primary_href(repomd_xml: &str) -> Result<String> {
    let mut reader = Reader::from_str(repomd_xml);
    reader.config_mut().trim_text(true);
    let mut in_primary = false;

    loop {
        match reader.read_event().context("reading Fedora repomd.xml")? {
            Event::Start(event) if event.name().as_ref() == b"data" => {
                in_primary = attr_value(&reader, &event, b"type")?.as_deref() == Some("primary");
            }
            Event::End(event) if event.name().as_ref() == b"data" => {
                in_primary = false;
            }
            Event::Empty(event) | Event::Start(event)
                if in_primary && event.name().as_ref() == b"location" =>
            {
                if let Some(href) = attr_value(&reader, &event, b"href")? {
                    return Ok(href);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    bail!("Fedora repomd.xml did not contain primary metadata location");
}

pub fn parse_primary_metadata(primary_xml: &str) -> Result<Vec<FedoraPackageCandidate>> {
    let mut reader = Reader::from_str(primary_xml);
    reader.config_mut().trim_text(true);
    let mut packages = Vec::new();
    let mut current = PrimaryPackageBuilder::default();
    let mut current_text: Option<PrimaryTextField> = None;
    let mut in_package = false;

    loop {
        match reader
            .read_event()
            .context("reading Fedora primary metadata")?
        {
            Event::Start(event) if event.name().as_ref() == b"package" => {
                current = PrimaryPackageBuilder::default();
                in_package = true;
            }
            Event::End(event) if event.name().as_ref() == b"package" => {
                in_package = false;
                if let Some(candidate) = std::mem::take(&mut current).finish()? {
                    packages.push(candidate);
                }
            }
            Event::Start(event) if in_package && event.name().as_ref() == b"name" => {
                current_text = Some(PrimaryTextField::Name);
            }
            Event::Start(event) if in_package && event.name().as_ref() == b"arch" => {
                current_text = Some(PrimaryTextField::Architecture);
            }
            Event::End(event)
                if event.name().as_ref() == b"name" || event.name().as_ref() == b"arch" =>
            {
                current_text = None;
            }
            Event::Empty(event) | Event::Start(event)
                if in_package && event.name().as_ref() == b"version" =>
            {
                let epoch = attr_value(&reader, &event, b"epoch")?.unwrap_or_else(|| "0".into());
                let version = attr_value(&reader, &event, b"ver")?
                    .ok_or_else(|| anyhow!("Fedora primary package version missing ver"))?;
                let release = attr_value(&reader, &event, b"rel")?
                    .ok_or_else(|| anyhow!("Fedora primary package version missing rel"))?;
                current.version = Some(format!("{epoch}:{version}-{release}"));
            }
            Event::Empty(event) | Event::Start(event)
                if in_package && event.name().as_ref() == b"location" =>
            {
                current.location_href = attr_value(&reader, &event, b"href")?;
            }
            Event::Text(event) if in_package => {
                if let Some(field) = current_text {
                    let text = event
                        .decode()
                        .context("decoding Fedora primary metadata text")?
                        .into_owned();
                    match field {
                        PrimaryTextField::Name => current.name = Some(text),
                        PrimaryTextField::Architecture => current.architecture = Some(text),
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(packages)
}

pub fn select_kernel_packages(
    packages: &[FedoraPackageCandidate],
    package_names: &[String],
    architecture: Option<Architecture>,
    max_packages: Option<usize>,
) -> Vec<FedoraPackageCandidate> {
    let mut selected = packages
        .iter()
        .filter(|package| package_names.iter().any(|name| name == &package.name))
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

pub fn extract_kernel_configs_from_rpm(rpm_bytes: &[u8]) -> Result<Vec<(String, String)>> {
    let mut reader = BufReader::new(Cursor::new(rpm_bytes));
    let package = rpm::Package::parse(&mut reader).context("parsing RPM package")?;
    let mut candidates = Vec::new();

    for file in package.files().context("reading RPM payload")? {
        let file = file.context("reading RPM payload file")?;
        let path = file.metadata.path.display().to_string();
        if !is_kernel_config_path(&path) || file.content.is_empty() {
            continue;
        }

        let config_text = String::from_utf8(file.content)
            .with_context(|| format!("decoding kernel config {path} as UTF-8"))?;
        candidates.push((path, config_text));
    }

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    candidates.sort_by(|(left, _), (right, _)| {
        kernel_config_path_priority(left).cmp(&kernel_config_path_priority(right))
    });
    Ok(vec![candidates.remove(0)])
}

#[derive(Debug, Default)]
struct PrimaryPackageBuilder {
    name: Option<String>,
    version: Option<String>,
    architecture: Option<String>,
    location_href: Option<String>,
}

impl PrimaryPackageBuilder {
    fn finish(self) -> Result<Option<FedoraPackageCandidate>> {
        let Some(name) = self.name else {
            return Ok(None);
        };
        let Some(version) = self.version else {
            return Ok(None);
        };
        let Some(architecture) = self.architecture else {
            return Ok(None);
        };
        let Some(location_href) = self.location_href else {
            return Ok(None);
        };

        Ok(Some(FedoraPackageCandidate {
            name,
            version,
            architecture: architecture.parse().map_err(anyhow::Error::msg)?,
            location_href,
        }))
    }
}

#[derive(Debug, Clone, Copy)]
enum PrimaryTextField {
    Name,
    Architecture,
}

fn attr_value(
    reader: &Reader<&[u8]>,
    event: &BytesStart<'_>,
    key: &[u8],
) -> Result<Option<String>> {
    for attr in event.attributes() {
        let attr = attr.context("reading XML attribute")?;
        if attr.key.as_ref() == key {
            return attr
                .decode_and_unescape_value(reader.decoder())
                .map(|value| Some(value.into_owned()))
                .context("decoding XML attribute");
        }
    }
    Ok(None)
}

fn decode_repo_metadata(bytes: &[u8], href: &str) -> Result<String> {
    let mut decoded = String::new();
    if href.ends_with(".gz") {
        GzDecoder::new(Cursor::new(bytes))
            .read_to_string(&mut decoded)
            .context("decompressing Fedora gzip metadata")?;
    } else if href.ends_with(".zst") || href.ends_with(".zstd") {
        zstd::stream::read::Decoder::new(Cursor::new(bytes))
            .context("initializing Fedora zstd metadata decoder")?
            .read_to_string(&mut decoded)
            .context("decompressing Fedora zstd metadata")?;
    } else {
        decoded = String::from_utf8(bytes.to_vec()).context("decoding Fedora metadata as UTF-8")?;
    }
    Ok(decoded)
}

fn is_kernel_config_path(path: &str) -> bool {
    let normalized = path.trim_start_matches("./").trim_start_matches('/');
    let segments: Vec<_> = normalized.split('/').collect();
    if segments.len() < 2 {
        return false;
    }

    if segments[segments.len() - 2] == "boot" && segments[segments.len() - 1].starts_with("config-")
    {
        return true;
    }

    (segments.len() >= 3
        && segments[0] == "lib"
        && segments[1] == "modules"
        && segments[segments.len() - 1] == "config")
        || (segments.len() >= 4
            && segments[0] == "usr"
            && segments[1] == "lib"
            && segments[2] == "modules"
            && segments[segments.len() - 1] == "config")
}

fn kernel_config_path_priority(path: &str) -> u8 {
    if path.contains("/lib/modules/") && path.ends_with("/config") {
        0
    } else {
        1
    }
}

fn fedora_repo_root(mirror: &str, release: &str, architecture: &Architecture) -> String {
    let arch = fedora_architecture_segment(architecture);
    if release == "rawhide" {
        format!("{mirror}/development/rawhide/Everything/{arch}/os")
    } else {
        format!("{mirror}/releases/{release}/Everything/{arch}/os")
    }
}

fn fedora_architecture_segment(architecture: &Architecture) -> &str {
    match architecture {
        Architecture::Amd64 => "x86_64",
        Architecture::Arm64 => "aarch64",
        Architecture::Armhf => "armhfp",
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
    use rpm::{BuildConfig, CompressionType, FileOptions, PackageBuilder};
    use std::io::Write;

    #[test]
    fn maps_known_architectures_to_fedora_repo_paths() {
        let mirror = "https://example.invalid";
        let amd64 = FedoraIndexerConfig::from_mirror(mirror, "42", [Architecture::Amd64]);
        let arm64 = FedoraIndexerConfig::from_mirror(mirror, "42", [Architecture::Arm64]);
        let ppc64el = FedoraIndexerConfig::from_mirror(mirror, "42", [Architecture::Ppc64el]);

        let amd64_repomd = match &amd64.feeds[0].repomd {
            FedoraMetadataLocation::Url(url) => url,
            FedoraMetadataLocation::Path(_) => panic!("expected URL repomd"),
        };
        let arm64_repomd = match &arm64.feeds[0].repomd {
            FedoraMetadataLocation::Url(url) => url,
            FedoraMetadataLocation::Path(_) => panic!("expected URL repomd"),
        };
        let ppc64el_repomd = match &ppc64el.feeds[0].repomd {
            FedoraMetadataLocation::Url(url) => url,
            FedoraMetadataLocation::Path(_) => panic!("expected URL repomd"),
        };

        assert!(amd64_repomd.contains("/Everything/x86_64/os/repodata/repomd.xml"));
        assert!(arm64_repomd.contains("/Everything/aarch64/os/repodata/repomd.xml"));
        assert!(ppc64el_repomd.contains("/Everything/ppc64le/os/repodata/repomd.xml"));
    }

    #[test]
    fn parses_primary_href_from_repomd() {
        let href = parse_primary_href(
            r#"<repomd>
  <data type="filelists"><location href="repodata/filelists.xml.zst"/></data>
  <data type="primary"><location href="repodata/primary.xml.zst"/></data>
</repomd>"#,
        )
        .expect("primary href");

        assert_eq!(href, "repodata/primary.xml.zst");
    }

    #[test]
    fn parses_and_selects_fedora_kernel_packages_from_primary_metadata() {
        let packages = parse_primary_metadata(
            r#"<metadata>
  <package type="rpm">
    <name>kernel-core</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="6.12.0" rel="1.fc99"/>
    <location href="Packages/k/kernel-core-6.12.0-1.fc99.x86_64.rpm"/>
  </package>
  <package type="rpm">
    <name>bash</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="5.2" rel="1.fc99"/>
    <location href="Packages/b/bash.rpm"/>
  </package>
</metadata>"#,
        )
        .expect("primary metadata");
        let selected = select_kernel_packages(
            &packages,
            &["kernel-core".to_string()],
            Some(Architecture::Amd64),
            None,
        );

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "kernel-core");
        assert_eq!(selected[0].version, "0:6.12.0-1.fc99");
        assert_eq!(selected[0].architecture, Architecture::Amd64);
    }

    #[test]
    fn decodes_gzip_primary_metadata() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"<metadata/>").expect("write gzip");
        let bytes = encoder.finish().expect("finish gzip");

        assert_eq!(
            decode_repo_metadata(&bytes, "repodata/primary.xml.gz").expect("decode"),
            "<metadata/>"
        );
    }

    #[test]
    fn decodes_zstd_primary_metadata() {
        let bytes = zstd::encode_all(b"<metadata/>" as &[u8], 0).expect("write zstd");

        assert_eq!(
            decode_repo_metadata(&bytes, "repodata/primary.xml.zst").expect("decode"),
            "<metadata/>"
        );
    }

    #[test]
    fn parses_primary_href_from_repomd_with_content_hash() {
        let href = parse_primary_href(
            r#"<repomd>
  <data type="primary"><location href="repodata/3310ccb5a66770d47f5659647701a5e8d11907f91ff599c231f6af2505fbc886-primary.xml.zst"/></data>
</repomd>"#,
        )
        .expect("primary href");

        assert_eq!(
            href,
            "repodata/3310ccb5a66770d47f5659647701a5e8d11907f91ff599c231f6af2505fbc886-primary.xml.zst"
        );
    }

    #[test]
    fn extracts_boot_config_from_rpm() {
        let rpm = minimal_rpm_with_config("CONFIG_BPF=y\n", "/boot/config-6.12.0-1.fc99.x86_64");

        let configs = extract_kernel_configs_from_rpm(&rpm).expect("extract configs");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "/boot/config-6.12.0-1.fc99.x86_64");
        assert!(configs[0].1.contains("CONFIG_BPF=y"));
    }

    #[test]
    fn prefers_lib_modules_config_over_empty_boot_placeholder() {
        let mut package_builder =
            PackageBuilder::new("kernel-core", "6.19.10", "MIT", "x86_64", "kernel");
        package_builder
            .release("300.fc44")
            .using_config(BuildConfig::v4().compression(CompressionType::Gzip))
            .with_file_contents([], FileOptions::new("/boot/config-6.19.10-300.fc44.x86_64"))
            .expect("add boot placeholder")
            .with_file_contents(
                b"CONFIG_BPF=y\n",
                FileOptions::new("/lib/modules/6.19.10-300.fc44.x86_64/config"),
            )
            .expect("add modules config");
        let package = package_builder.build().expect("build rpm");
        let mut rpm = Vec::new();
        package.write(&mut rpm).expect("write rpm");

        let configs = extract_kernel_configs_from_rpm(&rpm).expect("extract configs");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "/lib/modules/6.19.10-300.fc44.x86_64/config");
        assert!(configs[0].1.contains("CONFIG_BPF=y"));
    }

    #[test]
    fn extracts_usr_lib_modules_config_from_rpm() {
        let rpm =
            minimal_rpm_with_config("CONFIG_BPF=y\n", "/usr/lib/modules/7.0.9-1-default/config");

        let configs = extract_kernel_configs_from_rpm(&rpm).expect("extract configs");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "/usr/lib/modules/7.0.9-1-default/config");
        assert!(configs[0].1.contains("CONFIG_BPF=y"));
    }

    fn minimal_rpm_with_config(config: &str, rpm_path: &str) -> Vec<u8> {
        let mut package_builder =
            PackageBuilder::new("kernel-core", "6.12.0", "MIT", "x86_64", "kernel");
        package_builder
            .release("1.fc99")
            .using_config(BuildConfig::v4().compression(CompressionType::Gzip))
            .with_file_contents(config.as_bytes(), FileOptions::new(rpm_path))
            .expect("add config");
        let package = package_builder.build().expect("build rpm");
        let mut bytes = Vec::new();
        package.write(&mut bytes).expect("write rpm");
        bytes
    }
}
