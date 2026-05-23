use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::de;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::indexer::KernelConfigPackage;

pub const INDEX_SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Distribution {
    Android,
    Alpine,
    ArchLinux,
    CachyOS,
    Debian,
    EweOS,
    Fedora,
    AlmaLinux,
    CentOS,
    Kali,
    OpenAnolis,
    OpenEuler,
    OpenSUSE,
    Parabola,
    Proxmox,
    Rhel,
    Rocky,
    Ubuntu,
    Deepin,
    Kylin,
    AoscOS,
    Other(String),
}

impl Distribution {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Android => "android",
            Self::Alpine => "alpine",
            Self::ArchLinux => "archlinux",
            Self::CachyOS => "cachyos",
            Self::Debian => "debian",
            Self::EweOS => "eweos",
            Self::Fedora => "fedora",
            Self::AlmaLinux => "almalinux",
            Self::CentOS => "centos",
            Self::Kali => "kali",
            Self::OpenAnolis => "openanolis",
            Self::OpenEuler => "openeuler",
            Self::OpenSUSE => "opensuse",
            Self::Parabola => "parabola",
            Self::Proxmox => "proxmox",
            Self::Rhel => "rhel",
            Self::Rocky => "rocky",
            Self::Ubuntu => "ubuntu",
            Self::Deepin => "deepin",
            Self::Kylin => "kylin",
            Self::AoscOS => "aoscos",
            Self::Other(value) => value.as_str(),
        }
    }
}

impl fmt::Display for Distribution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Distribution {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let normalized = input.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err("distribution cannot be empty".to_string());
        }

        Ok(match normalized.as_str() {
            "android" => Self::Android,
            "alpine" | "alpine-linux" => Self::Alpine,
            "arch" | "archlinux" | "arch-linux" => Self::ArchLinux,
            "cachyos" | "cachy-os" => Self::CachyOS,
            "debian" => Self::Debian,
            "eweos" | "ewe-os" => Self::EweOS,
            "fedora" => Self::Fedora,
            "alma" | "almalinux" | "alma-linux" => Self::AlmaLinux,
            "centos" | "centos-stream" => Self::CentOS,
            "kali" => Self::Kali,
            "openanolis" | "open-anolis" | "anolis" => Self::OpenAnolis,
            "openeuler" | "open-euler" => Self::OpenEuler,
            "opensuse" | "open-suse" | "suse" => Self::OpenSUSE,
            "parabola" => Self::Parabola,
            "proxmox" | "pve" => Self::Proxmox,
            "rhel" | "redhat" | "red-hat" => Self::Rhel,
            "rocky" | "rockylinux" | "rocky-linux" => Self::Rocky,
            "ubuntu" => Self::Ubuntu,
            "deepin" => Self::Deepin,
            "kylin" | "kylinos" => Self::Kylin,
            "aosc" | "aoscos" | "aosc-os" => Self::AoscOS,
            other => Self::Other(other.to_string()),
        })
    }
}

impl Serialize for Distribution {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Distribution {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Architecture {
    Amd64,
    Arm64,
    Armhf,
    I386,
    Ppc64el,
    Riscv64,
    S390x,
    Other(String),
}

impl Architecture {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Amd64 => "amd64",
            Self::Arm64 => "arm64",
            Self::Armhf => "armhf",
            Self::I386 => "i386",
            Self::Ppc64el => "ppc64el",
            Self::Riscv64 => "riscv64",
            Self::S390x => "s390x",
            Self::Other(value) => value.as_str(),
        }
    }
}

impl fmt::Display for Architecture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Architecture {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let normalized = input.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err("architecture cannot be empty".to_string());
        }

        Ok(match normalized.as_str() {
            "amd64" | "x86_64" => Self::Amd64,
            "arm64" | "aarch64" => Self::Arm64,
            "armhf" | "armv7" | "armv7h" => Self::Armhf,
            "i386" | "x86" => Self::I386,
            "ppc64el" => Self::Ppc64el,
            "riscv64" => Self::Riscv64,
            "s390x" => Self::S390x,
            other => Self::Other(other.to_string()),
        })
    }
}

impl Serialize for Architecture {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Architecture {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigValue {
    BuiltIn,
    Module,
    Missing,
    Other(String),
}

impl ConfigValue {
    pub fn as_display_value(&self) -> &str {
        match self {
            Self::BuiltIn => "y",
            Self::Module => "m",
            Self::Missing => "-",
            Self::Other(value) => value.as_str(),
        }
    }
}

impl Serialize for ConfigValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::BuiltIn => serializer.serialize_str("built_in"),
            Self::Module => serializer.serialize_str("module"),
            Self::Missing => serializer.serialize_str("-"),
            Self::Other(value) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("other", value)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for ConfigValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawConfigValue {
            String(String),
            Other { other: String },
        }

        match RawConfigValue::deserialize(deserializer)? {
            RawConfigValue::String(value) => match value.as_str() {
                "built_in" => Ok(Self::BuiltIn),
                "module" => Ok(Self::Module),
                "-" => Ok(Self::Missing),
                other => Err(de::Error::custom(format!(
                    "unknown config value string {other:?}"
                ))),
            },
            RawConfigValue::Other { other } => Ok(Self::Other(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageKernel {
    pub version: String,
    pub architecture: Architecture,
    pub config_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageConfigOccurrence {
    pub kernel: String,
    pub value: ConfigValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageIndex {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub distribution: Distribution,
    pub package_name: String,
    pub kernels: BTreeMap<String, PackageKernel>,
    pub entries: BTreeMap<String, Vec<PackageConfigOccurrence>>,
}

impl PackageIndex {
    pub fn new(distribution: Distribution, package_name: impl Into<String>) -> Self {
        Self {
            schema_version: INDEX_SCHEMA_VERSION,
            generated_at: Utc::now(),
            distribution,
            package_name: package_name.into(),
            kernels: BTreeMap::new(),
            entries: BTreeMap::new(),
        }
    }

    pub fn from_packages(packages: impl IntoIterator<Item = KernelConfigPackage>) -> Self {
        let mut packages = packages.into_iter();
        let first = packages
            .next()
            .expect("PackageIndex::from_packages requires at least one package");
        let mut index = Self::new(first.distribution.clone(), first.package_name.clone());
        index.add_package(first);
        for package in packages {
            index.add_package(package);
        }
        index
    }

    pub fn add_package(&mut self, package: KernelConfigPackage) {
        debug_assert_eq!(self.distribution, package.distribution);
        debug_assert_eq!(self.package_name, package.package_name);

        let kernel = kernel_id(&package.package_version, &package.architecture);
        self.remove_kernel(&kernel);

        let config_path = config_relative_path(&package.package_version, &package.architecture);
        self.kernels.insert(
            kernel.clone(),
            PackageKernel {
                version: package.package_version.clone(),
                architecture: package.architecture.clone(),
                config_path,
                source: package.source.clone(),
            },
        );

        for (name, value) in parse_kernel_config(&package.config_text) {
            self.entries
                .entry(name)
                .or_default()
                .push(PackageConfigOccurrence {
                    kernel: kernel.clone(),
                    value,
                });
        }
    }

    pub fn remove_kernel(&mut self, kernel: &str) {
        self.kernels.remove(kernel);
        self.entries.retain(|_, occurrences| {
            occurrences.retain(|occurrence| occurrence.kernel != kernel);
            !occurrences.is_empty()
        });
    }

    pub fn sort_entries(&mut self) {
        for occurrences in self.entries.values_mut() {
            occurrences.sort_by(|left, right| left.kernel.cmp(&right.kernel));
        }
    }
}

pub fn write_packages_to_data_dir(
    packages: impl IntoIterator<Item = KernelConfigPackage>,
    data_dir: impl AsRef<Path>,
) -> Result<Vec<PathBuf>> {
    let data_dir = data_dir.as_ref();
    let mut groups: BTreeMap<(Distribution, String), Vec<KernelConfigPackage>> = BTreeMap::new();

    for package in packages {
        groups
            .entry((package.distribution.clone(), package.package_name.clone()))
            .or_default()
            .push(package);
    }

    let mut written_indexes = Vec::new();
    for ((distribution, package_name), packages) in groups {
        let distribution_segment = path_segment(distribution.as_str(), "distribution")?;
        let package_segment = path_segment(&package_name, "package")?;
        let package_dir = data_dir.join(distribution_segment).join(package_segment);
        fs::create_dir_all(&package_dir)
            .with_context(|| format!("creating package directory {}", package_dir.display()))?;
        let index_path = package_dir.join("index.json");
        let mut index = read_or_create_package_index(&index_path, distribution, package_name)?;

        for package in packages {
            let version_segment = path_segment(&package.package_version, "version")?;
            let architecture_segment = path_segment(package.architecture.as_str(), "architecture")?;
            let config_dir = package_dir.join(version_segment).join(architecture_segment);
            fs::create_dir_all(&config_dir)
                .with_context(|| format!("creating config directory {}", config_dir.display()))?;
            fs::write(config_dir.join("config"), &package.config_text)
                .with_context(|| format!("writing {}", config_dir.join("config").display()))?;
            index.add_package(package);
        }

        index.sort_entries();
        let json =
            serde_json::to_string_pretty(&index).context("serializing package config index")?;
        fs::write(&index_path, json)
            .with_context(|| format!("writing {}", index_path.display()))?;
        written_indexes.push(index_path);
    }

    Ok(written_indexes)
}

fn read_or_create_package_index(
    index_path: &Path,
    distribution: Distribution,
    package_name: String,
) -> Result<PackageIndex> {
    if !index_path.exists() {
        return Ok(PackageIndex::new(distribution, package_name));
    }

    let json = fs::read_to_string(index_path)
        .with_context(|| format!("reading existing package index {}", index_path.display()))?;
    let mut index: PackageIndex = serde_json::from_str(&json)
        .with_context(|| format!("parsing existing package index {}", index_path.display()))?;

    if index.distribution != distribution {
        bail!(
            "existing package index {} has distribution {}, expected {}",
            index_path.display(),
            index.distribution,
            distribution
        );
    }
    if index.package_name != package_name {
        bail!(
            "existing package index {} has package {}, expected {}",
            index_path.display(),
            index.package_name,
            package_name
        );
    }

    index.schema_version = INDEX_SCHEMA_VERSION;
    index.generated_at = Utc::now();
    Ok(index)
}

pub fn kernel_id(version: &str, architecture: &Architecture) -> String {
    format!("{version}/{}", architecture.as_str())
}

pub fn config_relative_path(version: &str, architecture: &Architecture) -> String {
    format!("{version}/{}/config", architecture.as_str())
}

fn path_segment<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
    {
        bail!("invalid {label} path segment {value:?}");
    }
    Ok(value)
}

pub fn normalize_config_name(input: &str) -> String {
    let trimmed = input.trim().to_ascii_uppercase();
    if trimmed.starts_with("CONFIG_") {
        trimmed
    } else {
        format!("CONFIG_{trimmed}")
    }
}

pub fn parse_kernel_config(config_text: &str) -> BTreeMap<String, ConfigValue> {
    let mut entries = BTreeMap::new();

    for line in config_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("##") {
            continue;
        }

        if let Some(name) = line
            .strip_prefix("# ")
            .and_then(|line| line.strip_suffix(" is not set"))
        {
            if name.starts_with("CONFIG_") {
                entries.insert(name.to_string(), ConfigValue::Missing);
            }
            continue;
        }

        let Some((name, raw_value)) = line.split_once('=') else {
            continue;
        };

        if !name.starts_with("CONFIG_") {
            continue;
        }

        let value = match raw_value {
            "y" => ConfigValue::BuiltIn,
            "m" => ConfigValue::Module,
            other => ConfigValue::Other(other.to_string()),
        };

        entries.insert(name.to_string(), value);
    }

    entries
}

pub fn parse_enabled_kernel_config(config_text: &str) -> BTreeMap<String, ConfigValue> {
    parse_kernel_config(config_text)
        .into_iter()
        .filter(|(_, value)| *value != ConfigValue::Missing)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_values_and_missing_entries() {
        let parsed = parse_kernel_config(
            r#"
CONFIG_BPF=y
CONFIG_NF_CONNTRACK=m
CONFIG_CMDLINE="console=ttyS0"
# CONFIG_UNUSED is not set
NOT_A_CONFIG=y
"#,
        );

        assert_eq!(parsed.get("CONFIG_BPF"), Some(&ConfigValue::BuiltIn));
        assert_eq!(
            parsed.get("CONFIG_NF_CONNTRACK"),
            Some(&ConfigValue::Module)
        );
        assert_eq!(
            parsed.get("CONFIG_CMDLINE"),
            Some(&ConfigValue::Other("\"console=ttyS0\"".to_string()))
        );
        assert_eq!(parsed.get("CONFIG_UNUSED"), Some(&ConfigValue::Missing));
        assert!(!parsed.contains_key("NOT_A_CONFIG"));
    }

    #[test]
    fn keeps_compatibility_helper_for_enabled_only_entries() {
        let parsed = parse_enabled_kernel_config("CONFIG_BPF=y\n# CONFIG_UNUSED is not set\n");

        assert!(parsed.contains_key("CONFIG_BPF"));
        assert!(!parsed.contains_key("CONFIG_UNUSED"));
    }

    #[test]
    fn builds_package_index_from_kernel_config_packages_without_entry_metadata_duplication() {
        let package = KernelConfigPackage {
            distribution: Distribution::Debian,
            package_name: "linux-image-amd64".to_string(),
            package_version: "6.1.0-1".to_string(),
            architecture: Architecture::Amd64,
            source: Some("https://example.invalid/linux-image.deb".to_string()),
            config_text: "CONFIG_BPF=y\nCONFIG_EXT4_FS=m\n# CONFIG_UNUSED is not set\n".to_string(),
        };

        let index = PackageIndex::from_packages([package]);

        let bpf = index.entries.get("CONFIG_BPF").expect("CONFIG_BPF entry");
        assert_eq!(bpf.len(), 1);
        assert_eq!(index.distribution, Distribution::Debian);
        assert_eq!(index.package_name, "linux-image-amd64");
        assert_eq!(bpf[0].kernel, "6.1.0-1/amd64");
        assert_eq!(bpf[0].value, ConfigValue::BuiltIn);
        assert_eq!(
            index.kernels["6.1.0-1/amd64"].config_path,
            "6.1.0-1/amd64/config"
        );

        let missing = index
            .entries
            .get("CONFIG_UNUSED")
            .expect("CONFIG_UNUSED entry");
        assert_eq!(missing[0].value, ConfigValue::Missing);
    }

    #[test]
    fn normalizes_user_supplied_config_names() {
        assert_eq!(normalize_config_name("bpf"), "CONFIG_BPF");
        assert_eq!(normalize_config_name(" config_ext4_fs "), "CONFIG_EXT4_FS");
    }

    #[test]
    fn serializes_typed_distribution_and_architecture_as_strings() {
        let kernel = PackageKernel {
            version: "6.1.0-1".to_string(),
            architecture: Architecture::Amd64,
            config_path: "6.1.0-1/amd64/config".to_string(),
            source: None,
        };

        let json = serde_json::to_string(&kernel).expect("serialize kernel");

        assert!(json.contains(r#""architecture":"amd64""#));
        assert!(
            serde_json::to_string(&ConfigValue::Missing)
                .expect("serialize value")
                .contains(r#""-""#)
        );
    }

    #[test]
    fn writes_data_tree_with_raw_config_and_package_index() {
        let package = KernelConfigPackage {
            distribution: Distribution::Debian,
            package_name: "linux-image-amd64".to_string(),
            package_version: "6.1.0-1".to_string(),
            architecture: Architecture::Amd64,
            source: None,
            config_text: "CONFIG_BPF=y\n".to_string(),
        };
        let temp = tempfile::tempdir().expect("tempdir");

        let indexes = write_packages_to_data_dir([package], temp.path()).expect("write data");

        assert_eq!(indexes.len(), 1);
        assert!(
            temp.path()
                .join("debian/linux-image-amd64/6.1.0-1/amd64/config")
                .exists()
        );
        assert!(
            temp.path()
                .join("debian/linux-image-amd64/index.json")
                .exists()
        );
    }

    #[test]
    fn merges_existing_package_index_when_writing_more_architectures() {
        let temp = tempfile::tempdir().expect("tempdir");
        let amd64 = KernelConfigPackage {
            distribution: Distribution::Debian,
            package_name: "linux-image-<VERSION>-<ARCH>".to_string(),
            package_version: "6.1.0-1".to_string(),
            architecture: Architecture::Amd64,
            source: None,
            config_text: "CONFIG_BPF=y\n".to_string(),
        };
        let riscv64 = KernelConfigPackage {
            distribution: Distribution::Debian,
            package_name: "linux-image-<VERSION>-<ARCH>".to_string(),
            package_version: "6.1.0-1".to_string(),
            architecture: Architecture::Riscv64,
            source: None,
            config_text: "CONFIG_BPF=y\n".to_string(),
        };

        write_packages_to_data_dir([amd64], temp.path()).expect("write amd64");
        write_packages_to_data_dir([riscv64], temp.path()).expect("write riscv64");

        let index_path = temp
            .path()
            .join("debian/linux-image-<VERSION>-<ARCH>/index.json");
        let index: PackageIndex = serde_json::from_str(
            &fs::read_to_string(&index_path).expect("read merged package index"),
        )
        .expect("parse merged package index");
        let bpf = index.entries.get("CONFIG_BPF").expect("CONFIG_BPF entry");

        assert!(index.kernels.contains_key("6.1.0-1/amd64"));
        assert!(index.kernels.contains_key("6.1.0-1/riscv64"));
        assert_eq!(bpf.len(), 2);
    }

    #[test]
    fn replaces_existing_kernel_entries_when_reindexing_same_kernel() {
        let temp = tempfile::tempdir().expect("tempdir");
        let package = KernelConfigPackage {
            distribution: Distribution::Debian,
            package_name: "linux-image-<VERSION>-<ARCH>".to_string(),
            package_version: "6.1.0-1".to_string(),
            architecture: Architecture::Amd64,
            source: None,
            config_text: "CONFIG_BPF=y\n".to_string(),
        };
        let updated_package = KernelConfigPackage {
            config_text: "CONFIG_EXT4_FS=m\n".to_string(),
            ..package.clone()
        };

        write_packages_to_data_dir([package], temp.path()).expect("write package");
        write_packages_to_data_dir([updated_package], temp.path()).expect("rewrite package");

        let index_path = temp
            .path()
            .join("debian/linux-image-<VERSION>-<ARCH>/index.json");
        let index: PackageIndex = serde_json::from_str(
            &fs::read_to_string(&index_path).expect("read rewritten package index"),
        )
        .expect("parse rewritten package index");

        assert!(!index.entries.contains_key("CONFIG_BPF"));
        assert_eq!(
            index
                .entries
                .get("CONFIG_EXT4_FS")
                .expect("CONFIG_EXT4_FS entry")
                .len(),
            1
        );
    }
}
