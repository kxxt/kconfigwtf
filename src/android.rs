use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;

use crate::ikconfig::looks_like_html;
pub use crate::ikconfig::{extract_ikconfig_from_image, looks_like_kernel_config};
use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage};

const DEFAULT_TARGET: &str = "kernel_aarch64";
const DEFAULT_CONFIG_ARTIFACT: &str = "kernel_aarch64_dot_config";
const BUILD_INFO_ARTIFACT: &str = "BUILD_INFO";
/// Kernel image artifacts to try with `extract-ikconfig`, in priority order.
const IKCONFIG_IMAGE_ARTIFACTS: &[&str] = &[
    "boot.img",
    "boot-gz.img",
    "vmlinux",
    "Image",
    "boot-lz4.img",
];
const CI_CONFIG_TARGETS: &[&str] = &["kernel_aarch64", "kernel_debug_aarch64"];
pub const DEFAULT_DISCOVERY_URL: &str =
    "https://source.android.com/docs/core/architecture/kernel/gki1-overview";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidReleaseBuildsLocation {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidArtifactBase {
    Ci,
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidGkiIndexerConfig {
    pub branch: String,
    pub release_builds: AndroidReleaseBuildsLocation,
    pub artifact_base: AndroidArtifactBase,
    pub target: String,
    pub config_artifact: String,
    pub architecture: Architecture,
    pub max_builds: Option<usize>,
}

impl AndroidGkiIndexerConfig {
    pub fn from_branch(branch: impl Into<String>) -> Self {
        let branch = branch.into();
        let url = android_release_builds_url(&branch);
        Self {
            branch,
            release_builds: AndroidReleaseBuildsLocation::Url(url),
            artifact_base: AndroidArtifactBase::Ci,
            target: DEFAULT_TARGET.to_string(),
            config_artifact: DEFAULT_CONFIG_ARTIFACT.to_string(),
            architecture: Architecture::Arm64,
            max_builds: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AndroidGkiIndexer {
    config: AndroidGkiIndexerConfig,
    client: reqwest::Client,
}

impl AndroidGkiIndexer {
    pub fn new(config: AndroidGkiIndexerConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    async fn load_release_builds(&self) -> Result<String> {
        match &self.config.release_builds {
            AndroidReleaseBuildsLocation::Url(url) => {
                let response = self
                    .client
                    .get(url)
                    .send()
                    .await
                    .with_context(|| format!("requesting Android GKI release builds {url}"))?
                    .error_for_status()
                    .with_context(|| {
                        format!("Android GKI release builds returned an error: {url}")
                    })?;
                response
                    .text()
                    .await
                    .with_context(|| format!("reading Android GKI release builds {url}"))
            }
            AndroidReleaseBuildsLocation::Path(path) => tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("reading Android GKI release builds {}", path.display())),
        }
    }

    async fn load_config(&self, build_id: &str) -> Result<(String, String)> {
        match &self.config.artifact_base {
            AndroidArtifactBase::Ci => self.load_config_from_ci(build_id).await,
            AndroidArtifactBase::Path(root) => self.load_config_from_local(root, build_id).await,
        }
    }

    async fn load_config_from_ci(&self, build_id: &str) -> Result<(String, String)> {
        let mut errors = Vec::new();
        for target in ci_targets_to_try(&self.config.target) {
            match self.load_config_from_ci_target(build_id, target).await {
                Ok(config) => return Ok(config),
                Err(error) => errors.push(format!("{target}: {error:#}")),
            }
        }

        bail!(
            "Android CI build {build_id} did not provide a kernel config from any CI target ({})",
            errors.join("; ")
        );
    }

    async fn load_config_from_ci_target(
        &self,
        build_id: &str,
        target: &str,
    ) -> Result<(String, String)> {
        let artifacts = self.load_ci_artifact_list(build_id, target).await?;
        let candidates =
            select_config_artifact_candidates(&artifacts, &self.config.config_artifact);
        if candidates.is_empty() {
            bail!(
                "no config artifacts listed in BUILD_INFO (available: {})",
                summarize_artifacts(&artifacts)
            );
        }

        let mut errors = Vec::new();
        for candidate in candidates {
            match self
                .load_config_from_ci_candidate(build_id, target, &candidate)
                .await
            {
                Ok(config) => return Ok(config),
                Err(error) => errors.push(format!("{candidate}: {error:#}")),
            }
        }

        bail!(
            "config artifacts were listed but could not be retrieved ({})",
            errors.join("; ")
        );
    }

    async fn load_config_from_ci_candidate(
        &self,
        build_id: &str,
        target: &str,
        artifact: &str,
    ) -> Result<(String, String)> {
        let url = android_ci_raw_artifact_url(build_id, target, artifact);
        if artifact == self.config.config_artifact {
            let text = self
                .download_ci_text(&url)
                .await
                .with_context(|| format!("downloading Android GKI config {url}"))?;
            if looks_like_kernel_config(&text) {
                return Ok((url, text));
            }
            bail!("downloaded {artifact} but it was not a kernel config");
        }

        let bytes = self
            .download_ci_bytes(&url)
            .await
            .with_context(|| format!("downloading Android GKI kernel image {url}"))?;
        let config_text = extract_ikconfig_from_image(&bytes)
            .with_context(|| format!("extracting IKCONFIG from {url}"))?;
        Ok((format!("{url}#ikconfig"), config_text))
    }

    async fn load_config_from_local(
        &self,
        root: &Path,
        build_id: &str,
    ) -> Result<(String, String)> {
        let artifact_dir = root.join(build_id).join(&self.config.target);
        let candidates =
            local_config_artifact_candidates(&artifact_dir, &self.config.config_artifact);
        if candidates.is_empty() {
            bail!(
                "Android artifacts for build {build_id} under {} did not provide a kernel config",
                artifact_dir.display()
            );
        }

        let mut errors = Vec::new();
        for candidate in candidates {
            match self
                .load_config_from_local_candidate(&artifact_dir, &candidate)
                .await
            {
                Ok(config) => return Ok(config),
                Err(error) => errors.push(format!("{candidate}: {error:#}")),
            }
        }

        bail!(
            "Android artifacts for build {build_id} under {} could not be read ({})",
            artifact_dir.display(),
            errors.join("; ")
        );
    }

    async fn load_config_from_local_candidate(
        &self,
        artifact_dir: &Path,
        artifact: &str,
    ) -> Result<(String, String)> {
        let path = artifact_dir.join(artifact);
        if artifact == self.config.config_artifact {
            let text = tokio::fs::read_to_string(&path)
                .await
                .with_context(|| format!("reading Android GKI config {}", path.display()))?;
            if looks_like_kernel_config(&text) {
                return Ok((path.display().to_string(), text));
            }
            bail!("{} was not a kernel config", path.display());
        }

        let bytes = tokio::fs::read(&path)
            .await
            .with_context(|| format!("reading Android GKI kernel image {}", path.display()))?;
        let config_text = extract_ikconfig_from_image(&bytes)
            .with_context(|| format!("extracting IKCONFIG from {}", path.display()))?;
        Ok((format!("{}#ikconfig", path.display()), config_text))
    }

    async fn load_ci_artifact_list(&self, build_id: &str, target: &str) -> Result<Vec<String>> {
        let url = android_ci_raw_artifact_url(build_id, target, BUILD_INFO_ARTIFACT);
        let text = self
            .download_ci_text(&url)
            .await
            .with_context(|| format!("downloading Android CI BUILD_INFO {url}"))?;
        parse_build_info_artifacts(&text)
    }

    async fn download_ci_text(&self, url: &str) -> Result<String> {
        let bytes = self.download_ci_bytes(url).await?;
        String::from_utf8(bytes).with_context(|| format!("decoding Android CI artifact {url}"))
    }

    async fn download_ci_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("requesting Android CI artifact {url}"))?
            .error_for_status()
            .with_context(|| format!("Android CI artifact returned an error: {url}"))?;
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .with_context(|| format!("reading Android CI artifact {url}"))
    }
}

#[async_trait]
impl KernelConfigIndexer for AndroidGkiIndexer {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>> {
        let release_builds = self.load_release_builds().await?;
        let metadata = parse_release_builds(&release_builds)?;
        let releases = select_releases(&metadata, self.config.max_builds);

        if releases.is_empty() {
            bail!(
                "Android GKI release metadata for {} did not contain any builds",
                self.config.branch
            );
        }

        let mut packages = Vec::new();
        for release in releases {
            let (source, config_text) = self.load_config(&release.kernel_bid).await?;
            if !looks_like_kernel_config(&config_text) {
                bail!(
                    "Android GKI build {} ({}) did not produce a kernel config (source: {source})",
                    release.kernel_bid,
                    release.tag
                );
            }
            packages.push(KernelConfigPackage {
                distribution: Distribution::Android,
                package_name: metadata.name.clone(),
                package_version: release.tag.clone(),
                architecture: self.config.architecture.clone(),
                source: Some(source),
                config_text,
            });
        }

        Ok(packages)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AndroidReleaseBuilds {
    pub name: String,
    pub branches: Vec<AndroidReleaseBranch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AndroidReleaseBranch {
    pub name: String,
    pub kernel_version: String,
    pub releases: Vec<AndroidRelease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AndroidRelease {
    pub tag: String,
    pub date: String,
    pub sha1: String,
    pub kernel_bid: String,
}

#[derive(Debug, Deserialize)]
struct AndroidBuildInfo {
    target: AndroidBuildInfoTarget,
}

#[derive(Debug, Deserialize)]
struct AndroidBuildInfoTarget {
    dir_list: Vec<String>,
}

pub fn parse_release_builds(input: &str) -> Result<AndroidReleaseBuilds> {
    let json = extract_json_object(input)?;
    serde_json::from_str(json).context("parsing Android GKI release builds JSON")
}

pub fn parse_build_info_artifacts(input: &str) -> Result<Vec<String>> {
    if looks_like_html(input) {
        bail!("Android CI BUILD_INFO response was HTML instead of JSON");
    }
    let build_info: AndroidBuildInfo =
        serde_json::from_str(input).context("parsing Android CI BUILD_INFO JSON")?;
    Ok(build_info.target.dir_list)
}

pub fn discover_release_build_branches(input: &str) -> Vec<String> {
    let mut branches = Vec::new();
    for segment in input.split("gki-").skip(1) {
        let Some((slug, rest)) = segment.split_once("-release-builds") else {
            continue;
        };
        if !is_android_gki_slug(slug) {
            continue;
        }

        let branch = slug.replace('_', ".");
        if branch.contains("-deprecated") || rest.starts_with("-deprecated") {
            continue;
        }
        if !branches.contains(&branch) {
            branches.push(branch);
        }
    }
    branches.sort();
    branches.reverse();
    branches
}

pub fn select_releases(
    metadata: &AndroidReleaseBuilds,
    max_builds: Option<usize>,
) -> Vec<AndroidRelease> {
    let mut releases = metadata
        .branches
        .iter()
        .flat_map(|branch| branch.releases.iter().cloned())
        .collect::<Vec<_>>();

    releases.sort_by(|left, right| {
        (&right.date, &right.tag, &right.kernel_bid).cmp(&(&left.date, &left.tag, &left.kernel_bid))
    });

    if let Some(max) = max_builds {
        releases.truncate(max);
    }

    releases
}

pub fn android_release_builds_url(branch: &str) -> String {
    let slug = branch.replace('.', "_");
    format!(
        "https://source.android.com/docs/core/architecture/kernel/gki-{slug}-release-builds.json"
    )
}

pub fn android_ci_raw_artifact_url(build_id: &str, target: &str, artifact: &str) -> String {
    format!("https://ci.android.com/builds/submitted/{build_id}/{target}/latest/raw/{artifact}")
}

pub fn select_config_artifact_candidates(artifacts: &[String], dot_config: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if artifacts.iter().any(|artifact| artifact == dot_config) {
        candidates.push(dot_config.to_string());
    }
    for &artifact in IKCONFIG_IMAGE_ARTIFACTS {
        if artifacts.iter().any(|listed| listed == artifact) {
            candidates.push(artifact.to_string());
        }
    }
    candidates
}

fn local_config_artifact_candidates(artifact_dir: &Path, dot_config: &str) -> Vec<String> {
    let mut listed = Vec::new();
    if artifact_dir.join(dot_config).is_file() {
        listed.push(dot_config.to_string());
    }
    for &artifact in IKCONFIG_IMAGE_ARTIFACTS {
        if artifact_dir.join(artifact).is_file() {
            listed.push(artifact.to_string());
        }
    }
    listed
}

fn ci_targets_to_try(preferred: &str) -> Vec<&str> {
    let mut targets = vec![preferred];
    for &target in CI_CONFIG_TARGETS {
        if target != preferred && !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

fn summarize_artifacts(artifacts: &[String]) -> String {
    const KEYWORDS: &[&str] = &["config", "boot", "Image", "vmlinux"];
    let relevant: Vec<_> = artifacts
        .iter()
        .filter(|artifact| KEYWORDS.iter().any(|keyword| artifact.contains(keyword)))
        .take(8)
        .map(String::as_str)
        .collect();
    if relevant.is_empty() {
        return format!("{} files", artifacts.len());
    }
    relevant.join(", ")
}

fn extract_json_object(input: &str) -> Result<&str> {
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, character) in input.char_indices() {
        if start.is_none() {
            if character == '{' {
                start = Some(index);
                depth = 1;
            }
            continue;
        }

        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let start = start.expect("start set");
                    return Ok(&input[start..=index]);
                }
            }
            _ => {}
        }
    }

    bail!("Android GKI release builds did not contain a JSON object")
}

fn is_android_gki_slug(input: &str) -> bool {
    input
        .strip_prefix("android")
        .is_some_and(|rest| rest.contains('-') && rest.chars().all(is_gki_slug_character))
}

fn is_gki_slug_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_builds_from_json_page_html() {
        let metadata = parse_release_builds(
            r#"<p>The following JSON provides data:</p>
<devsite-code><pre><code>
{
  "name": "android16-6.12",
  "branches": [
    {
      "name": "android16-6.12-2025-06",
      "kernel_version": "6.12.23",
      "releases": [
        {
          "tag": "android16-6.12-2025-06_r1",
          "date": "2025-06-12",
          "sha1": "2d954fcf3d1b73a41d0fa498324da357ec96cbdf",
          "kernel_bid": "13586339"
        }
      ]
    }
  ]
}
</code></pre></devsite-code>"#,
        )
        .expect("parse metadata");

        assert_eq!(metadata.name, "android16-6.12");
        assert_eq!(metadata.branches[0].releases[0].kernel_bid, "13586339");
    }

    #[test]
    fn selects_newest_releases_first() {
        let metadata = AndroidReleaseBuilds {
            name: "android16-6.12".to_string(),
            branches: vec![AndroidReleaseBranch {
                name: "android16-6.12-2025-06".to_string(),
                kernel_version: "6.12.23".to_string(),
                releases: vec![
                    release("android16-6.12-2025-06_r1", "2025-06-12", "1"),
                    release("android16-6.12-2025-06_r2", "2025-06-25", "2"),
                ],
            }],
        };

        let selected = select_releases(&metadata, Some(1));

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].tag, "android16-6.12-2025-06_r2");
    }

    #[test]
    fn builds_android_urls() {
        assert_eq!(
            android_release_builds_url("android16-6.12"),
            "https://source.android.com/docs/core/architecture/kernel/gki-android16-6_12-release-builds.json"
        );
        assert_eq!(
            android_ci_raw_artifact_url("15035146", "kernel_aarch64", "kernel_aarch64_dot_config"),
            "https://ci.android.com/builds/submitted/15035146/kernel_aarch64/latest/raw/kernel_aarch64_dot_config"
        );
    }

    #[test]
    fn discovers_release_build_branches_from_overview_html() {
        let branches = discover_release_build_branches(
            r#"
<a href="/docs/core/architecture/kernel/gki-android16-6_12-release-builds">android16-6.12 Releases</a>
<a href="/docs/core/architecture/kernel/gki-android15-6_6-release-builds">android15-6.6 Releases</a>
<a href="/docs/core/architecture/kernel/gki-android16-6_12-deprecated-builds">deprecated</a>
"#,
        );

        assert_eq!(branches, vec!["android16-6.12", "android15-6.6"]);
    }

    #[test]
    fn rejects_html_as_kernel_config() {
        assert!(!looks_like_kernel_config(
            "<!DOCTYPE html><html><body><artifact-page></artifact-page></body></html>"
        ));
        assert!(looks_like_kernel_config(
            "# Linux/arm64 6.12.23 Kernel Configuration\nCONFIG_BPF=y\n"
        ));
    }

    #[test]
    fn parses_build_info_artifact_list() {
        let artifacts = parse_build_info_artifacts(
            r#"{"target":{"dir_list":["boot.img","kernel_aarch64_dot_config"]}}"#,
        )
        .expect("parse BUILD_INFO");

        assert!(artifacts.contains(&"boot.img".to_string()));
        assert!(artifacts.contains(&"kernel_aarch64_dot_config".to_string()));
    }

    #[test]
    fn selects_vmlinux_when_boot_image_is_missing() {
        let artifacts = parse_build_info_artifacts(
            r#"{"target":{"dir_list":["vmlinux","Image","System.map"]}}"#,
        )
        .expect("parse BUILD_INFO");
        let candidates = select_config_artifact_candidates(&artifacts, DEFAULT_CONFIG_ARTIFACT);

        assert_eq!(candidates, vec!["vmlinux".to_string(), "Image".to_string()]);
    }

    #[test]
    fn prefers_dot_config_and_boot_image_before_vmlinux() {
        let artifacts = parse_build_info_artifacts(
            r#"{"target":{"dir_list":["vmlinux","boot.img","kernel_aarch64_dot_config"]}}"#,
        )
        .expect("parse BUILD_INFO");
        let candidates = select_config_artifact_candidates(&artifacts, DEFAULT_CONFIG_ARTIFACT);

        assert_eq!(
            candidates,
            vec![
                "kernel_aarch64_dot_config".to_string(),
                "boot.img".to_string(),
                "vmlinux".to_string(),
            ]
        );
    }

    #[test]
    fn extracts_ikconfig_from_boot_image_when_available() {
        let boot_img = PathBuf::from("/tmp/boot.img");
        if !boot_img.is_file() {
            return;
        }

        let bytes = std::fs::read(&boot_img).expect("read boot.img");
        let config = extract_ikconfig_from_image(&bytes).expect("extract ikconfig");

        assert!(config.contains("CONFIG_"));
        assert!(config.lines().count() > 1000);
    }

    #[test]
    fn extracts_ikconfig_from_vmlinux_when_available() {
        let vmlinux = PathBuf::from("/tmp/vmlinux-15260231");
        if !vmlinux.is_file() {
            return;
        }

        let bytes = std::fs::read(&vmlinux).expect("read vmlinux");
        let config = extract_ikconfig_from_image(&bytes).expect("extract ikconfig");

        assert!(config.contains("CONFIG_"));
        assert!(config.contains("Kernel Configuration"));
    }

    fn release(tag: &str, date: &str, build_id: &str) -> AndroidRelease {
        AndroidRelease {
            tag: tag.to_string(),
            date: date.to_string(),
            sha1: "sha1".to_string(),
            kernel_bid: build_id.to_string(),
        }
    }
}
