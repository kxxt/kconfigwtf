use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::de;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::indexer::KernelConfigPackage;

pub const INDEX_SCHEMA_VERSION: u32 = 6;
pub const DEFAULT_MAX_INDEX_BYTES: usize = 50 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Distribution {
    Android,
    Alpine,
    ArchLinux,
    CachyOS,
    ChromeOS,
    Debian,
    EweOS,
    Fedora,
    Guix,
    AlmaLinux,
    CentOS,
    Kali,
    OpenAnolis,
    OpenEuler,
    OpenKylin,
    OpenWrt,
    OpenSUSE,
    NixOS,
    Parabola,
    Proxmox,
    Rhel,
    Rocky,
    Ubuntu,
    Deepin,
    Kylin,
    AoscOS,
    OracleLinux,
    AmazonLinux,
    AzureLinux,
    Slackware,
    Void,
    Other(String),
}

impl Distribution {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Android => "android",
            Self::Alpine => "alpine",
            Self::ArchLinux => "archlinux",
            Self::CachyOS => "cachyos",
            Self::ChromeOS => "chromeos",
            Self::Debian => "debian",
            Self::EweOS => "eweos",
            Self::Fedora => "fedora",
            Self::Guix => "guix",
            Self::AlmaLinux => "almalinux",
            Self::CentOS => "centos",
            Self::Kali => "kali",
            Self::OpenAnolis => "openanolis",
            Self::OpenEuler => "openeuler",
            Self::OpenKylin => "openkylin",
            Self::OpenWrt => "openwrt",
            Self::OpenSUSE => "opensuse",
            Self::NixOS => "nixos",
            Self::Parabola => "parabola",
            Self::Proxmox => "proxmox",
            Self::Rhel => "rhel",
            Self::Rocky => "rocky",
            Self::Ubuntu => "ubuntu",
            Self::Deepin => "deepin",
            Self::Kylin => "kylin",
            Self::AoscOS => "aoscos",
            Self::OracleLinux => "oraclelinux",
            Self::AmazonLinux => "amazonlinux",
            Self::AzureLinux => "azurelinux",
            Self::Slackware => "slackware",
            Self::Void => "void",
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
            "chromeos" | "chrome-os" | "chromiumos" | "chromium-os" => Self::ChromeOS,
            "debian" => Self::Debian,
            "eweos" | "ewe-os" => Self::EweOS,
            "fedora" => Self::Fedora,
            "guix" | "guix-system" => Self::Guix,
            "alma" | "almalinux" | "alma-linux" => Self::AlmaLinux,
            "centos" | "centos-stream" => Self::CentOS,
            "kali" => Self::Kali,
            "openanolis" | "open-anolis" | "anolis" => Self::OpenAnolis,
            "openeuler" | "open-euler" => Self::OpenEuler,
            "openkylin" | "open-kylin" => Self::OpenKylin,
            "openwrt" | "open-wrt" => Self::OpenWrt,
            "opensuse" | "open-suse" | "suse" => Self::OpenSUSE,
            "nixos" | "nix-os" => Self::NixOS,
            "parabola" => Self::Parabola,
            "proxmox" | "pve" => Self::Proxmox,
            "rhel" | "redhat" | "red-hat" => Self::Rhel,
            "rocky" | "rockylinux" | "rocky-linux" => Self::Rocky,
            "ubuntu" => Self::Ubuntu,
            "deepin" => Self::Deepin,
            "kylin" | "kylinos" => Self::Kylin,
            "aosc" | "aoscos" | "aosc-os" => Self::AoscOS,
            "oracle" | "oraclelinux" | "oracle-linux" | "ol" => Self::OracleLinux,
            "amazon" | "amazonlinux" | "amazon-linux" | "al" => Self::AmazonLinux,
            "azure" | "azurelinux" | "azure-linux" => Self::AzureLinux,
            "slackware" => Self::Slackware,
            "void" | "voidlinux" | "void-linux" => Self::Void,
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
            "ppc64el" | "ppc64le" => Self::Ppc64el,
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
    #[serde(default = "unknown_release_label")]
    pub release: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageIndex {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub distribution: Distribution,
    pub package_name: String,
    pub kernels: BTreeMap<String, PackageKernel>,
    pub entries: BTreeMap<String, Vec<PackageConfigOccurrence>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LegacyPackageIndex {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub distribution: Distribution,
    pub package_name: String,
    pub kernels: BTreeMap<String, PackageKernel>,
    pub entries: BTreeMap<String, Vec<PackageConfigOccurrence>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CompactPackageKernel {
    pub version: String,
    pub release: usize,
    pub architecture: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct CompactConfigEntry {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub built_in: Vec<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module: Vec<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other: Vec<(usize, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CompactPackageIndex {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub distribution: Distribution,
    pub package_name: String,
    pub releases: Vec<String>,
    pub architectures: Vec<Architecture>,
    pub kernels: Vec<CompactPackageKernel>,
    pub entries: BTreeMap<String, CompactConfigEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum RawPackageIndex {
    Compact(CompactPackageIndex),
    Legacy(LegacyPackageIndex),
}

impl Serialize for PackageIndex {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        CompactPackageIndex::from_package_index(self)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PackageIndex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawPackageIndex::deserialize(deserializer)?;
        match raw {
            RawPackageIndex::Compact(index) => index.into_package_index(),
            RawPackageIndex::Legacy(index) => Ok(PackageIndex::from_legacy(index)),
        }
        .map_err(de::Error::custom)
    }
}

impl CompactPackageIndex {
    fn from_package_index(index: &PackageIndex) -> Result<Self> {
        let releases = index
            .kernels
            .values()
            .map(|kernel| kernel.release.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let architectures = index
            .kernels
            .values()
            .map(|kernel| kernel.architecture.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let release_indexes = releases
            .iter()
            .enumerate()
            .map(|(index, release)| (release.clone(), index))
            .collect::<BTreeMap<_, _>>();
        let architecture_indexes = architectures
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, architecture)| (architecture, index))
            .collect::<BTreeMap<_, _>>();
        let kernel_indexes = index
            .kernels
            .keys()
            .cloned()
            .enumerate()
            .map(|(index, kernel)| (kernel, index))
            .collect::<BTreeMap<_, _>>();

        let kernels = index
            .kernels
            .values()
            .map(|kernel| {
                let release = *release_indexes.get(&kernel.release).ok_or_else(|| {
                    anyhow::anyhow!("missing release index for {}", kernel.release)
                })?;
                let architecture =
                    *architecture_indexes
                        .get(&kernel.architecture)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "missing architecture index for {}",
                                kernel.architecture
                            )
                        })?;
                Ok(CompactPackageKernel {
                    version: kernel.version.clone(),
                    release,
                    architecture,
                    source: kernel.source.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mut entries = BTreeMap::new();
        for (name, occurrences) in &index.entries {
            let mut entry = CompactConfigEntry::default();
            for occurrence in occurrences {
                let kernel = *kernel_indexes.get(&occurrence.kernel).ok_or_else(|| {
                    anyhow::anyhow!(
                        "entry {name} references unknown kernel {}",
                        occurrence.kernel
                    )
                })?;
                match &occurrence.value {
                    ConfigValue::BuiltIn => entry.built_in.push(kernel),
                    ConfigValue::Module => entry.module.push(kernel),
                    ConfigValue::Other(value) => entry.other.push((kernel, value.clone())),
                    ConfigValue::Missing => entry.missing.push(kernel),
                }
            }
            entry.built_in.sort_unstable();
            entry.module.sort_unstable();
            entry
                .other
                .sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
            entries.insert(name.clone(), entry);
        }

        Ok(Self {
            schema_version: INDEX_SCHEMA_VERSION,
            generated_at: index.generated_at,
            distribution: index.distribution.clone(),
            package_name: index.package_name.clone(),
            releases,
            architectures,
            kernels,
            entries,
        })
    }

    fn into_package_index(self) -> Result<PackageIndex> {
        let mut kernels = BTreeMap::new();
        let mut kernel_ids = Vec::with_capacity(self.kernels.len());

        for kernel in self.kernels {
            let release = self
                .releases
                .get(kernel.release)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("invalid release index {}", kernel.release))?;
            let architecture = self
                .architectures
                .get(kernel.architecture)
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!("invalid architecture index {}", kernel.architecture)
                })?;
            let version = kernel.version;
            let kernel_id = kernel_id(&version, &architecture);
            kernel_ids.push(kernel_id.clone());
            kernels.insert(
                kernel_id,
                PackageKernel {
                    version: version.clone(),
                    release,
                    architecture: architecture.clone(),
                    config_path: config_relative_path(&version, &architecture),
                    source: kernel.source,
                },
            );
        }

        let mut entries = BTreeMap::new();
        for (name, entry) in self.entries {
            let mut occurrences = Vec::new();
            for kernel in entry.built_in {
                occurrences.push(PackageConfigOccurrence {
                    kernel: kernel_ids
                        .get(kernel)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("invalid kernel index {kernel}"))?,
                    value: ConfigValue::BuiltIn,
                });
            }
            for kernel in entry.module {
                occurrences.push(PackageConfigOccurrence {
                    kernel: kernel_ids
                        .get(kernel)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("invalid kernel index {kernel}"))?,
                    value: ConfigValue::Module,
                });
            }
            for (kernel, value) in entry.other {
                occurrences.push(PackageConfigOccurrence {
                    kernel: kernel_ids
                        .get(kernel)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("invalid kernel index {kernel}"))?,
                    value: ConfigValue::Other(value),
                });
            }
            occurrences.sort_by(|left, right| left.kernel.cmp(&right.kernel));
            entries.insert(name, occurrences);
        }

        Ok(PackageIndex {
            schema_version: INDEX_SCHEMA_VERSION,
            generated_at: self.generated_at,
            distribution: self.distribution,
            package_name: self.package_name,
            kernels,
            entries,
        })
    }
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
                release: package.release.clone(),
                architecture: package.architecture.clone(),
                config_path,
                source: package.source.clone(),
            },
        );

        for (name, value) in parse_kernel_config(&package.config_text) {
            let entry = self.entries.entry(name).or_default();
            if value != ConfigValue::Missing {
                entry.push(PackageConfigOccurrence {
                    kernel: kernel.clone(),
                    value,
                });
            }
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
            occurrences
                .dedup_by(|left, right| left.kernel == right.kernel && left.value == right.value);
        }
    }

    pub fn merge(&mut self, other: PackageIndex) -> Result<()> {
        if self.distribution != other.distribution {
            bail!(
                "cannot merge {} package index into {} package index",
                other.distribution,
                self.distribution
            );
        }
        if self.package_name != other.package_name {
            bail!(
                "cannot merge package {} into package {}",
                other.package_name,
                self.package_name
            );
        }

        let other_kernels = other.kernels.keys().cloned().collect::<BTreeSet<_>>();
        for kernel in &other_kernels {
            self.remove_kernel(kernel);
        }

        self.generated_at = self.generated_at.max(other.generated_at);
        self.kernels.extend(other.kernels);

        for (name, mut occurrences) in other.entries {
            let entry = self.entries.entry(name).or_default();
            entry.retain(|occurrence| !other_kernels.contains(&occurrence.kernel));
            entry.append(&mut occurrences);
        }

        self.schema_version = INDEX_SCHEMA_VERSION;
        self.sort_entries();
        Ok(())
    }

    fn from_legacy(index: LegacyPackageIndex) -> Self {
        let mut entries = BTreeMap::new();

        for (name, occurrences) in index.entries {
            entries.insert(
                name,
                occurrences
                    .into_iter()
                    .filter(|occurrence| occurrence.value != ConfigValue::Missing)
                    .collect(),
            );
        }

        let mut index = Self {
            schema_version: INDEX_SCHEMA_VERSION,
            generated_at: index.generated_at,
            distribution: index.distribution,
            package_name: index.package_name,
            kernels: index.kernels,
            entries,
        };
        index.sort_entries();
        index
    }

    fn release_names(&self) -> Vec<String> {
        self.kernels
            .values()
            .map(|kernel| kernel.release.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn subset_for_kernels(&self, selected_kernels: &BTreeSet<String>) -> Self {
        let kernels = self
            .kernels
            .iter()
            .filter(|(kernel, _)| selected_kernels.contains(*kernel))
            .map(|(kernel, value)| (kernel.clone(), value.clone()))
            .collect();
        let entries = self
            .entries
            .iter()
            .map(|(name, occurrences)| {
                (
                    name.clone(),
                    occurrences
                        .iter()
                        .filter(|occurrence| selected_kernels.contains(&occurrence.kernel))
                        .cloned()
                        .collect(),
                )
            })
            .collect();

        Self {
            schema_version: INDEX_SCHEMA_VERSION,
            generated_at: self.generated_at,
            distribution: self.distribution.clone(),
            package_name: self.package_name.clone(),
            kernels,
            entries,
        }
    }

    fn subset_for_release(&self, release: &str) -> Self {
        let kernels = self
            .kernels
            .iter()
            .filter(|(_, kernel)| kernel.release == release)
            .map(|(kernel, _)| kernel.clone())
            .collect::<BTreeSet<_>>();
        self.subset_for_kernels(&kernels)
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
        let mut index = read_or_create_package_index(&package_dir, distribution, package_name)?;

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
        written_indexes.extend(write_package_index_to_dir(
            &index,
            &package_dir,
            DEFAULT_MAX_INDEX_BYTES,
        )?);
    }

    Ok(written_indexes)
}

fn read_or_create_package_index(
    package_dir: &Path,
    distribution: Distribution,
    package_name: String,
) -> Result<PackageIndex> {
    let mut index_paths = list_package_index_files(package_dir)?;
    if index_paths.is_empty() {
        return Ok(PackageIndex::new(distribution, package_name));
    }

    index_paths.sort();
    let first = index_paths.remove(0);
    let mut index = read_package_index(&first)
        .with_context(|| format!("loading existing package index {}", first.display()))?;
    for index_path in index_paths {
        let shard = read_package_index(&index_path)
            .with_context(|| format!("loading package index shard {}", index_path.display()))?;
        index.merge(shard)?;
    }

    if index.distribution != distribution {
        bail!(
            "existing package index {} has distribution {}, expected {}",
            package_dir.display(),
            index.distribution,
            distribution
        );
    }
    if index.package_name != package_name {
        bail!(
            "existing package index {} has package {}, expected {}",
            package_dir.display(),
            index.package_name,
            package_name
        );
    }

    index.schema_version = INDEX_SCHEMA_VERSION;
    index.generated_at = Utc::now();
    Ok(index)
}

pub fn read_package_index(index_path: &Path) -> Result<PackageIndex> {
    let json = fs::read_to_string(index_path)
        .with_context(|| format!("reading {}", index_path.display()))?;
    let mut index = serde_json::from_str::<PackageIndex>(&json)
        .with_context(|| format!("parsing package index {}", index_path.display()))?;
    index.schema_version = INDEX_SCHEMA_VERSION;
    index.sort_entries();
    Ok(index)
}

pub fn is_package_index_file_name(name: &str) -> bool {
    name == "index.json" || (name.starts_with("index_") && name.ends_with(".json"))
}

pub fn list_package_index_files(package_dir: &Path) -> Result<Vec<PathBuf>> {
    if !package_dir.exists() {
        return Ok(Vec::new());
    }

    let mut indexes = Vec::new();
    for entry in fs::read_dir(package_dir)
        .with_context(|| format!("reading package directory {}", package_dir.display()))?
    {
        let entry = entry.with_context(|| format!("reading {}", package_dir.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading {}", entry.path().display()))?;
        if !file_type.is_file() {
            continue;
        }

        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if is_package_index_file_name(&name) {
            indexes.push(entry.path());
        }
    }

    indexes.sort();
    Ok(indexes)
}

pub fn write_package_index_to_dir(
    index: &PackageIndex,
    package_dir: &Path,
    max_bytes: usize,
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(package_dir)
        .with_context(|| format!("creating package directory {}", package_dir.display()))?;

    let shards = shard_package_index(index, max_bytes)?;
    let existing = list_package_index_files(package_dir)?;
    let mut written = Vec::new();
    let mut keep = BTreeSet::new();

    for shard in shards {
        let path = package_dir.join(&shard.file_name);
        fs::write(&path, shard.json).with_context(|| format!("writing {}", path.display()))?;
        keep.insert(path.clone());
        written.push(path);
    }

    for path in existing {
        if !keep.contains(&path) {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }

    written.sort();
    Ok(written)
}

pub fn kernel_id(version: &str, architecture: &Architecture) -> String {
    format!("{version}/{}", architecture.as_str())
}

pub fn config_relative_path(version: &str, architecture: &Architecture) -> String {
    format!("{version}/{}/config", architecture.as_str())
}

fn unknown_release_label() -> String {
    "unknown".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexShard {
    file_name: String,
    json: String,
}

fn shard_package_index(index: &PackageIndex, max_bytes: usize) -> Result<Vec<IndexShard>> {
    let full_json =
        serde_json::to_string_pretty(index).context("serializing package config index")?;
    if full_json.len() <= max_bytes {
        return Ok(vec![IndexShard {
            file_name: "index.json".to_string(),
            json: full_json,
        }]);
    }

    let releases = index.release_names();
    if releases.len() <= 1 {
        return split_index_subset(index, None, max_bytes);
    }

    let mut shards = Vec::new();
    for release in releases {
        let subset = index.subset_for_release(&release);
        shards.extend(split_index_subset(&subset, Some(&release), max_bytes)?);
    }
    Ok(shards)
}

fn split_index_subset(
    index: &PackageIndex,
    release: Option<&str>,
    max_bytes: usize,
) -> Result<Vec<IndexShard>> {
    let json = serde_json::to_string_pretty(index).context("serializing package config index")?;
    if json.len() <= max_bytes {
        return Ok(vec![IndexShard {
            file_name: shard_file_name(release, None)?,
            json,
        }]);
    }

    let kernel_ids = index.kernels.keys().cloned().collect::<Vec<_>>();
    if kernel_ids.len() <= 1 {
        bail!(
            "package index shard {} still exceeds {} bytes with a single kernel",
            shard_file_name(release, None)?,
            max_bytes
        );
    }

    let mut shards = Vec::new();
    let mut start = 0usize;
    let mut part = 1usize;
    while start < kernel_ids.len() {
        let mut low = start + 1;
        let mut high = kernel_ids.len();
        let mut best = None;

        while low <= high {
            let mid = low + (high - low) / 2;
            let selected = kernel_ids[start..mid]
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let subset = index.subset_for_kernels(&selected);
            let subset_json = serde_json::to_string_pretty(&subset)
                .context("serializing split package config index")?;
            if subset_json.len() <= max_bytes {
                best = Some((mid, subset_json));
                low = mid + 1;
            } else if mid == start + 1 {
                high = start;
            } else {
                high = mid - 1;
            }
        }

        let Some((end, json)) = best else {
            bail!(
                "package index shard {} could not be reduced under {} bytes",
                shard_file_name(release, Some(part))?,
                max_bytes
            );
        };

        shards.push(IndexShard {
            file_name: shard_file_name(release, Some(part))?,
            json,
        });
        start = end;
        part += 1;
    }

    if shards.len() == 1 {
        shards[0].file_name = shard_file_name(release, None)?;
    }

    Ok(shards)
}

fn shard_file_name(release: Option<&str>, part: Option<usize>) -> Result<String> {
    match (release, part) {
        (None, None) => Ok("index.json".to_string()),
        (None, Some(part)) => Ok(format!("index_part{part}.json")),
        (Some(release), None) => Ok(format!(
            "index_{}.json",
            path_segment(release, "release shard")?
        )),
        (Some(release), Some(part)) => Ok(format!(
            "index_{}_part{part}.json",
            path_segment(release, "release shard")?
        )),
    }
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
    fn parses_ppc64le_as_ppc64el_architecture() {
        assert_eq!(
            "ppc64le".parse::<Architecture>().expect("parse ppc64le"),
            Architecture::Ppc64el
        );
    }

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
            release: "trixie".to_string(),
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
        assert_eq!(index.kernels["6.1.0-1/amd64"].release, "trixie");
        assert_eq!(
            index.kernels["6.1.0-1/amd64"].config_path,
            "6.1.0-1/amd64/config"
        );

        let missing = index
            .entries
            .get("CONFIG_UNUSED")
            .expect("CONFIG_UNUSED entry");
        assert!(missing.is_empty());
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
            release: "trixie".to_string(),
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
    fn serializes_package_indexes_to_compact_schema() {
        let package = KernelConfigPackage {
            distribution: Distribution::Debian,
            release: "trixie".to_string(),
            package_name: "linux-image-amd64".to_string(),
            package_version: "6.1.0-1".to_string(),
            architecture: Architecture::Amd64,
            source: Some("https://example.invalid/linux-image.deb".to_string()),
            config_text: "CONFIG_BPF=y\n# CONFIG_UNUSED is not set\n".to_string(),
        };

        let index = PackageIndex::from_packages([package]);
        let json = serde_json::to_value(&index).expect("serialize package index");

        assert_eq!(json["schema_version"], serde_json::json!(6));
        assert!(json["releases"].is_array());
        assert!(json["architectures"].is_array());
        assert!(json["kernels"].is_array());
        assert!(json["entries"]["CONFIG_BPF"]["built_in"].is_array());
        assert!(json["entries"]["CONFIG_UNUSED"].is_object());
        assert_eq!(
            json["entries"]["CONFIG_UNUSED"]
                .as_object()
                .expect("unused entry object")
                .len(),
            0
        );
    }

    #[test]
    fn writes_data_tree_with_raw_config_and_package_index() {
        let package = KernelConfigPackage {
            distribution: Distribution::Debian,
            release: "trixie".to_string(),
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
    fn splits_large_indexes_by_release_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let package_dir = temp.path().join("debian/linux-image-amd64");
        let bookworm_config = (0..64)
            .map(|index| format!("CONFIG_BOOKWORM_{index}=y\n"))
            .collect::<String>();
        let trixie_config = (0..64)
            .map(|index| format!("CONFIG_TRIXIE_{index}=y\n"))
            .collect::<String>();
        let bookworm_package = KernelConfigPackage {
            distribution: Distribution::Debian,
            release: "bookworm".to_string(),
            package_name: "linux-image-amd64".to_string(),
            package_version: "6.1.0-1".to_string(),
            architecture: Architecture::Amd64,
            source: None,
            config_text: bookworm_config,
        };
        let trixie_package = KernelConfigPackage {
            distribution: Distribution::Debian,
            release: "trixie".to_string(),
            package_name: "linux-image-amd64".to_string(),
            package_version: "6.6.0-1".to_string(),
            architecture: Architecture::Amd64,
            source: None,
            config_text: trixie_config,
        };
        let index = PackageIndex::from_packages([bookworm_package.clone(), trixie_package.clone()]);
        let shard_max = serde_json::to_string_pretty(&index.subset_for_release("bookworm"))
            .expect("serialize bookworm shard")
            .len()
            .max(
                serde_json::to_string_pretty(&index.subset_for_release("trixie"))
                    .expect("serialize trixie shard")
                    .len(),
            );
        let full_len = serde_json::to_string_pretty(&index)
            .expect("serialize full index")
            .len();
        let max_bytes = shard_max + ((full_len - shard_max) / 2);

        let written = write_package_index_to_dir(&index, &package_dir, max_bytes)
            .expect("write split indexes");

        assert_eq!(written.len(), 2);
        assert!(package_dir.join("index_bookworm.json").exists());
        assert!(package_dir.join("index_trixie.json").exists());
        assert!(!package_dir.join("index.json").exists());
    }

    #[test]
    fn merges_existing_package_index_when_writing_more_architectures() {
        let temp = tempfile::tempdir().expect("tempdir");
        let amd64 = KernelConfigPackage {
            distribution: Distribution::Debian,
            release: "trixie".to_string(),
            package_name: "linux-image-<VERSION>-<ARCH>".to_string(),
            package_version: "6.1.0-1".to_string(),
            architecture: Architecture::Amd64,
            source: None,
            config_text: "CONFIG_BPF=y\n".to_string(),
        };
        let riscv64 = KernelConfigPackage {
            distribution: Distribution::Debian,
            release: "trixie".to_string(),
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
            release: "trixie".to_string(),
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
