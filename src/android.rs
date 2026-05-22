use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;

use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage};

const DEFAULT_TARGET: &str = "kernel_aarch64";
const DEFAULT_CONFIG_ARTIFACT: &str = "kernel_aarch64_dot_config";
const BOOT_IMAGE_ARTIFACT: &str = "boot.img";
const BUILD_INFO_ARTIFACT: &str = "BUILD_INFO";
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
        let artifacts = self.load_ci_artifact_list(build_id).await?;

        if artifacts.iter().any(|artifact| artifact == &self.config.config_artifact) {
            let url = android_ci_raw_artifact_url(
                build_id,
                &self.config.target,
                &self.config.config_artifact,
            );
            let text = self
                .download_ci_text(&url)
                .await
                .with_context(|| format!("downloading Android GKI config {url}"))?;
            if looks_like_kernel_config(&text) {
                return Ok((url, text));
            }
        }

        if artifacts.iter().any(|artifact| artifact == BOOT_IMAGE_ARTIFACT) {
            let url =
                android_ci_raw_artifact_url(build_id, &self.config.target, BOOT_IMAGE_ARTIFACT);
            let bytes = self
                .download_ci_bytes(&url)
                .await
                .with_context(|| format!("downloading Android GKI boot image {url}"))?;
            let config_text = extract_ikconfig_from_image(&bytes)
                .with_context(|| format!("extracting IKCONFIG from {url}"))?;
            return Ok((format!("{url}#ikconfig"), config_text));
        }

        bail!(
            "Android CI build {build_id} for target {} did not provide {} or {BOOT_IMAGE_ARTIFACT}",
            self.config.target,
            self.config.config_artifact
        );
    }

    async fn load_config_from_local(&self, root: &Path, build_id: &str) -> Result<(String, String)> {
        let artifact_dir = root.join(build_id).join(&self.config.target);
        let dot_config = artifact_dir.join(&self.config.config_artifact);
        if dot_config.is_file() {
            let text = tokio::fs::read_to_string(&dot_config)
                .await
                .with_context(|| format!("reading Android GKI config {}", dot_config.display()))?;
            if looks_like_kernel_config(&text) {
                return Ok((dot_config.display().to_string(), text));
            }
        }

        let boot_img = artifact_dir.join(BOOT_IMAGE_ARTIFACT);
        if boot_img.is_file() {
            let bytes = tokio::fs::read(&boot_img)
                .await
                .with_context(|| format!("reading Android GKI boot image {}", boot_img.display()))?;
            let config_text = extract_ikconfig_from_image(&bytes)
                .with_context(|| format!("extracting IKCONFIG from {}", boot_img.display()))?;
            return Ok((format!("{}#ikconfig", boot_img.display()), config_text));
        }

        bail!(
            "Android artifacts for build {build_id} under {} did not provide {} or {BOOT_IMAGE_ARTIFACT}",
            artifact_dir.display(),
            self.config.config_artifact
        );
    }

    async fn load_ci_artifact_list(&self, build_id: &str) -> Result<Vec<String>> {
        let url = android_ci_raw_artifact_url(build_id, &self.config.target, BUILD_INFO_ARTIFACT);
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

pub fn looks_like_kernel_config(text: &str) -> bool {
    if looks_like_html(text) || text.trim().is_empty() {
        return false;
    }

    text.lines().take(200).any(|line| {
        let line = line.trim();
        line.starts_with("CONFIG_")
            || line.starts_with("# CONFIG_")
            || line.contains("Kernel Configuration")
    })
}

pub fn extract_ikconfig_from_image(image: &[u8]) -> Result<String> {
    let script = locate_extract_ikconfig_script()?;
    let temp_path = std::env::temp_dir().join(format!(
        "kconfigwtf-boot-{}-{}.img",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&temp_path, image)
        .with_context(|| format!("writing temporary boot image {}", temp_path.display()))?;

    let output = Command::new("sh")
        .arg(&script)
        .arg(&temp_path)
        .output()
        .with_context(|| format!("running {}", script.display()))?;
    let _ = std::fs::remove_file(&temp_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "extract-ikconfig failed for {}: {stderr}",
            temp_path.display()
        );
    }

    let config = String::from_utf8(output.stdout).context("decoding extract-ikconfig stdout")?;
    if !looks_like_kernel_config(&config) {
        bail!("extract-ikconfig output did not look like a kernel config");
    }
    Ok(config)
}

fn locate_extract_ikconfig_script() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("EXTRACT_IKCONFIG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "EXTRACT_IKCONFIG is set but {} does not exist",
            path.display()
        );
    }

    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/extract-ikconfig");
    if bundled.is_file() {
        return Ok(bundled);
    }

    if let Ok(path) = which_extract_ikconfig() {
        return Ok(path);
    }

    bail!(
        "extract-ikconfig was not found; set EXTRACT_IKCONFIG or install the kernel script"
    );
}

fn which_extract_ikconfig() -> Result<PathBuf> {
    let output = Command::new("sh")
        .arg("-c")
        .arg("command -v extract-ikconfig")
        .output()
        .context("locating extract-ikconfig in PATH")?;
    if !output.status.success() {
        bail!("extract-ikconfig is not available in PATH");
    }
    let path = String::from_utf8(output.stdout)
        .context("decoding extract-ikconfig path")?
        .trim()
        .to_string();
    if path.is_empty() {
        bail!("extract-ikconfig is not available in PATH");
    }
    Ok(PathBuf::from(path))
}

fn looks_like_html(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<html")
        || trimmed.contains("<artifact-page")
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

    fn release(tag: &str, date: &str, build_id: &str) -> AndroidRelease {
        AndroidRelease {
            tag: tag.to_string(),
            date: date.to_string(),
            sha1: "sha1".to_string(),
            kernel_bid: build_id.to_string(),
        }
    }
}
