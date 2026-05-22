use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;

use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage};

const DEFAULT_TARGET: &str = "kernel_aarch64";
const DEFAULT_CONFIG_ARTIFACT: &str = "kernel_aarch64_dot_config";
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
            AndroidArtifactBase::Ci => {
                let url = android_ci_raw_artifact_url(
                    build_id,
                    &self.config.target,
                    &self.config.config_artifact,
                );
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("requesting Android GKI config {url}"))?
                    .error_for_status()
                    .with_context(|| format!("Android GKI config returned an error: {url}"))?;
                let text = response
                    .text()
                    .await
                    .with_context(|| format!("reading Android GKI config {url}"))?;
                Ok((url, text))
            }
            AndroidArtifactBase::Path(root) => {
                let path = root
                    .join(build_id)
                    .join(&self.config.target)
                    .join(&self.config.config_artifact);
                let text = tokio::fs::read_to_string(&path)
                    .await
                    .with_context(|| format!("reading Android GKI config {}", path.display()))?;
                Ok((path.display().to_string(), text))
            }
        }
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

pub fn parse_release_builds(input: &str) -> Result<AndroidReleaseBuilds> {
    let json = extract_json_object(input)?;
    serde_json::from_str(json).context("parsing Android GKI release builds JSON")
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

    fn release(tag: &str, date: &str, build_id: &str) -> AndroidRelease {
        AndroidRelease {
            tag: tag.to_string(),
            date: date.to_string(),
            sha1: "sha1".to_string(),
            kernel_bid: build_id.to_string(),
        }
    }
}
