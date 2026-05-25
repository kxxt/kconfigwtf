use anyhow::Result;
use async_trait::async_trait;

use crate::index::{Architecture, Distribution};

pub const ROLLING_RELEASE: &str = "rolling";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelConfigPackage {
    pub distribution: Distribution,
    pub release: String,
    pub package_name: String,
    pub package_version: String,
    pub architecture: Architecture,
    pub source: Option<String>,
    pub config_text: String,
}

pub fn rolling_release_label() -> String {
    ROLLING_RELEASE.to_string()
}

pub fn normalize_release_label(raw: &str) -> String {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        "unknown".to_string()
    } else {
        normalized
    }
}

pub fn normalize_apt_release_label(suite: &str) -> String {
    let normalized = normalize_release_label(suite);
    if normalized == "kali-rolling" || normalized == ROLLING_RELEASE {
        return rolling_release_label();
    }

    for suffix in ["-updates", "-security", "-backports", "-proposed"] {
        if let Some(base) = normalized.strip_suffix(suffix) {
            return base.to_string();
        }
    }

    normalized
}

pub fn normalize_alpine_release_label(release: &str) -> String {
    let normalized = normalize_release_label(release);
    if normalized == "edge" {
        rolling_release_label()
    } else {
        normalized
    }
}

pub fn normalize_rpm_release_label(distribution: &Distribution, release: &str) -> String {
    let normalized = normalize_release_label(release);
    if matches!(distribution, Distribution::OpenSUSE) && normalized == "tumbleweed" {
        rolling_release_label()
    } else {
        normalized
    }
}

pub fn normalize_slackware_release_label(release: &str) -> String {
    let normalized = normalize_release_label(release);
    if normalized == "current" || normalized.ends_with("-current") {
        return rolling_release_label();
    }

    normalized
        .rsplit_once('-')
        .map(|(_, tail)| tail.to_string())
        .unwrap_or(normalized)
}

pub fn normalize_openwrt_release_label(version_number: Option<&str>) -> String {
    match version_number {
        Some(version) if version.trim().eq_ignore_ascii_case("snapshot") => rolling_release_label(),
        Some(version) if !version.trim().is_empty() => normalize_release_label(version),
        _ => rolling_release_label(),
    }
}

pub fn normalize_nix_release_label(flake_ref: &str) -> String {
    let normalized = normalize_release_label(flake_ref);
    if normalized == "nixpkgs" || normalized.contains("unstable") {
        return rolling_release_label();
    }

    if let Some((_, suffix)) = normalized.rsplit_once("nixos-") {
        return suffix
            .split(['#', '?'])
            .next()
            .unwrap_or(suffix)
            .to_string();
    }

    if let Some((_, suffix)) = normalized.rsplit_once("release-") {
        return suffix
            .split(['#', '?'])
            .next()
            .unwrap_or(suffix)
            .to_string();
    }

    normalized
}

#[async_trait]
pub trait KernelConfigIndexer: Send + Sync {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>>;
}
