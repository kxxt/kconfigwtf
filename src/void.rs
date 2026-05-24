use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::StatusCode;
use serde::Deserialize;
use tokio::fs;

use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage};

pub const DEFAULT_VOID_GITHUB_API_TREE_URL: &str =
    "https://api.github.com/repos/void-linux/void-packages/git/trees/master?recursive=1";
pub const DEFAULT_VOID_GITHUB_RAW_SRCPKGS_URL: &str =
    "https://raw.githubusercontent.com/void-linux/void-packages/master/srcpkgs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoidPackageBase {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoidRepoFeed {
    pub distribution: Distribution,
    pub architecture: Architecture,
    /// Base URL or filesystem path to the `srcpkgs` tree.
    pub package_base: VoidPackageBase,
    /// Package recipe directories to consider (e.g. `linux6.6`).
    pub package_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoidIndexerConfig {
    pub feeds: Vec<VoidRepoFeed>,
    pub package_name_prefix: String,
    pub max_packages: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct VoidIndexer {
    config: VoidIndexerConfig,
    client: reqwest::Client,
}

impl VoidIndexer {
    pub fn new(config: VoidIndexerConfig) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("kconfigwtf/0.1")
            .build()
            .expect("construct reqwest client");
        Self { config, client }
    }

    async fn load_text(
        &self,
        base: &VoidPackageBase,
        package_name: &str,
        relative_path: &str,
    ) -> Result<(String, String)> {
        match self
            .load_optional_text(base, package_name, relative_path)
            .await?
        {
            Some(result) => Ok(result),
            None => bail!(
                "missing {} for Void package {}",
                relative_path,
                package_name
            ),
        }
    }

    async fn load_optional_text(
        &self,
        base: &VoidPackageBase,
        package_name: &str,
        relative_path: &str,
    ) -> Result<Option<(String, String)>> {
        match base {
            VoidPackageBase::Url(base_url) => {
                let url = join_srcpkgs_url(base_url, package_name, relative_path);
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("requesting Void source file {url}"))?;
                if response.status() == StatusCode::NOT_FOUND {
                    return Ok(None);
                }
                let response = response
                    .error_for_status()
                    .with_context(|| format!("Void source file returned an error: {url}"))?;
                let text = response
                    .text()
                    .await
                    .with_context(|| format!("reading Void source file {url}"))?;
                Ok(Some((url, text)))
            }
            VoidPackageBase::Path(root) => {
                let path = normalize_srcpkgs_root(root)
                    .join(package_name)
                    .join(relative_path);
                let text = match fs::read_to_string(&path).await {
                    Ok(text) => text,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("reading Void source file {}", path.display())
                        });
                    }
                };
                Ok(Some((path.display().to_string(), text)))
            }
        }
    }

    pub async fn discover_packages_from_github() -> Result<Vec<String>> {
        let client = reqwest::Client::builder()
            .user_agent("kconfigwtf/0.1")
            .build()
            .expect("construct reqwest client");
        let response = client
            .get(DEFAULT_VOID_GITHUB_API_TREE_URL)
            .send()
            .await
            .context("requesting Void GitHub source tree")?
            .error_for_status()
            .context("Void GitHub source tree returned an error")?;
        let tree: GithubTreeResponse = serde_json::from_str(
            &response
                .text()
                .await
                .context("reading Void GitHub source tree JSON")?,
        )
        .context("parsing Void GitHub source tree JSON")?;
        if tree.truncated {
            bail!("Void GitHub source tree response was truncated");
        }

        let mut packages_with_templates = BTreeSet::new();
        let mut packages_with_dotconfigs = BTreeSet::new();

        for entry in tree.tree {
            let Some(stripped) = entry.path.strip_prefix("srcpkgs/") else {
                continue;
            };
            let mut segments = stripped.split('/');
            let Some(package_name) = segments.next() else {
                continue;
            };
            let Some(next) = segments.next() else {
                continue;
            };

            if entry.kind == "blob" && next == "template" {
                packages_with_templates.insert(package_name.to_string());
                continue;
            }

            if entry.kind == "blob"
                && next == "files"
                && segments.next().is_some_and(|name| {
                    name.ends_with("-dotconfig") || name.ends_with("-dotconfig-custom")
                })
            {
                packages_with_dotconfigs.insert(package_name.to_string());
            }
        }

        Ok(packages_with_templates
            .intersection(&packages_with_dotconfigs)
            .cloned()
            .collect())
    }

    pub async fn discover_packages_from_path(root: &Path) -> Result<Vec<String>> {
        let srcpkgs_root = normalize_srcpkgs_root(root);
        let mut packages = BTreeSet::new();
        let mut entries = fs::read_dir(&srcpkgs_root)
            .await
            .with_context(|| format!("reading {}", srcpkgs_root.display()))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .with_context(|| format!("reading directory entry in {}", srcpkgs_root.display()))?
        {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let package_name = entry.file_name();
            let package_name = package_name.to_string_lossy().to_string();
            if !path.join("template").is_file() {
                continue;
            }
            if !has_local_dotconfig(&path).await? {
                continue;
            }
            packages.insert(package_name);
        }

        Ok(packages.into_iter().collect())
    }
}

#[async_trait]
impl KernelConfigIndexer for VoidIndexer {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>> {
        let mut packages = Vec::new();
        let mut selected_count = 0usize;

        for feed in &self.config.feeds {
            let mut candidates = feed
                .package_names
                .iter()
                .filter(|name| name.starts_with(&self.config.package_name_prefix))
                .cloned()
                .collect::<Vec<_>>();
            candidates.sort();
            if let Some(max) = self.config.max_packages {
                candidates.truncate(max);
            }

            selected_count += candidates.len();

            for package_name in candidates {
                let (template_source, template_text) = self
                    .load_text(&feed.package_base, &package_name, "template")
                    .await
                    .with_context(|| format!("loading template for Void package {package_name}"))?;
                let template =
                    parse_template(&template_text, &package_name).with_context(|| {
                        format!("parsing Void template for {package_name} from {template_source}")
                    })?;

                let mut found_config = false;
                for dotconfig_name in dotconfig_names_for_arch(&feed.architecture) {
                    let relative_path = format!("files/{dotconfig_name}");
                    let Some((source, config_text)) = self
                        .load_optional_text(&feed.package_base, &package_name, &relative_path)
                        .await
                        .with_context(|| {
                            format!("loading Void config {} for {}", relative_path, package_name)
                        })?
                    else {
                        continue;
                    };

                    found_config = true;
                    packages.push(KernelConfigPackage {
                        distribution: feed.distribution.clone(),
                        package_name: template.package_name.clone(),
                        package_version: format!("{}_{}", template.version, template.revision),
                        architecture: feed.architecture.clone(),
                        source: Some(source),
                        config_text,
                    });
                    break;
                }

                if !found_config {
                    eprintln!(
                        "skipping Void package {} for {}: no matching dotconfig file",
                        package_name, feed.architecture
                    );
                }
            }
        }

        if selected_count == 0 {
            bail!(
                "Void indexer did not find any candidate packages matching prefix {:?}",
                self.config.package_name_prefix
            );
        }

        if packages.is_empty() {
            bail!(
                "Void indexer selected {selected_count} package(s), but none had an architecture-specific dotconfig"
            );
        }

        Ok(packages)
    }
}

#[derive(Debug, Deserialize)]
struct GithubTreeResponse {
    truncated: bool,
    tree: Vec<GithubTreeEntry>,
}

#[derive(Debug, Deserialize)]
struct GithubTreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VoidTemplate {
    package_name: String,
    version: String,
    revision: String,
}

fn parse_template(template: &str, default_package_name: &str) -> Result<VoidTemplate> {
    let package_name = parse_template_assignment(template, "pkgname")
        .unwrap_or_else(|| default_package_name.to_string());
    let version = parse_template_assignment(template, "version")
        .context("missing version assignment in template")?;
    let revision = parse_template_assignment(template, "revision")
        .context("missing revision assignment in template")?;
    Ok(VoidTemplate {
        package_name,
        version,
        revision,
    })
}

fn parse_template_assignment(template: &str, key: &str) -> Option<String> {
    for line in template.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix(&format!("{key}=")) {
            return Some(unquote_shell_scalar(value.trim()));
        }
    }
    None
}

fn unquote_shell_scalar(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn join_srcpkgs_url(base_url: &str, package_name: &str, relative_path: &str) -> String {
    format!(
        "{}/{}/{}",
        base_url.trim_end_matches('/'),
        package_name.trim_matches('/'),
        relative_path.trim_start_matches('/')
    )
}

fn normalize_srcpkgs_root(root: &Path) -> PathBuf {
    let nested = root.join("srcpkgs");
    if nested.is_dir() {
        nested
    } else {
        root.to_path_buf()
    }
}

async fn has_local_dotconfig(package_dir: &Path) -> Result<bool> {
    let files_dir = package_dir.join("files");
    let mut entries = match fs::read_dir(&files_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", files_dir.display()));
        }
    };

    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("reading directory entry in {}", files_dir.display()))?
    {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with("-dotconfig") || name.ends_with("-dotconfig-custom") {
            return Ok(true);
        }
    }

    Ok(false)
}

fn dotconfig_names_for_arch(arch: &Architecture) -> Vec<String> {
    match arch {
        Architecture::Amd64 => vec![
            "x86_64-dotconfig-custom".to_string(),
            "x86_64-dotconfig".to_string(),
        ],
        Architecture::Arm64 => vec![
            "arm64-dotconfig-custom".to_string(),
            "arm64-dotconfig".to_string(),
            "aarch64-dotconfig-custom".to_string(),
            "aarch64-dotconfig".to_string(),
        ],
        Architecture::Armhf => vec![
            "armv7l-dotconfig-custom".to_string(),
            "armv7l-dotconfig".to_string(),
            "armv7-dotconfig-custom".to_string(),
            "armv7-dotconfig".to_string(),
            "arm-dotconfig-custom".to_string(),
            "arm-dotconfig".to_string(),
        ],
        Architecture::I386 => vec![
            "i386-dotconfig-custom".to_string(),
            "i386-dotconfig".to_string(),
            "i686-dotconfig-custom".to_string(),
            "i686-dotconfig".to_string(),
        ],
        Architecture::Ppc64el => vec![
            "ppc64le-dotconfig-custom".to_string(),
            "ppc64le-dotconfig".to_string(),
        ],
        Architecture::Riscv64 => vec![
            "riscv64-dotconfig-custom".to_string(),
            "riscv64-dotconfig".to_string(),
        ],
        Architecture::S390x => vec![
            "s390x-dotconfig-custom".to_string(),
            "s390x-dotconfig".to_string(),
        ],
        Architecture::Other(value) => vec![
            format!("{value}-dotconfig-custom"),
            format!("{value}-dotconfig"),
        ],
    }
}
