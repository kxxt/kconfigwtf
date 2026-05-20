use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::indexer::KernelConfigPackage;

pub const INDEX_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigValue {
    BuiltIn,
    Module,
    Other(String),
}

impl ConfigValue {
    pub fn as_display_value(&self) -> &str {
        match self {
            Self::BuiltIn => "y",
            Self::Module => "m",
            Self::Other(value) => value.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelConfigRecord {
    pub distribution: String,
    pub package_name: String,
    pub package_version: String,
    pub architecture: String,
    pub value: ConfigValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigIndex {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub entries: BTreeMap<String, Vec<KernelConfigRecord>>,
}

impl Default for ConfigIndex {
    fn default() -> Self {
        Self {
            schema_version: INDEX_SCHEMA_VERSION,
            generated_at: Utc::now(),
            entries: BTreeMap::new(),
        }
    }
}

impl ConfigIndex {
    pub fn from_packages(packages: impl IntoIterator<Item = KernelConfigPackage>) -> Self {
        let mut index = Self::default();
        for package in packages {
            index.add_package(package);
        }
        index
    }

    pub fn add_package(&mut self, package: KernelConfigPackage) {
        for (name, value) in parse_enabled_kernel_config(&package.config_text) {
            self.entries
                .entry(name)
                .or_default()
                .push(KernelConfigRecord {
                    distribution: package.distribution.clone(),
                    package_name: package.package_name.clone(),
                    package_version: package.package_version.clone(),
                    architecture: package.architecture.clone(),
                    source: package.source.clone(),
                    value,
                });
        }
    }

    pub fn sort_records(&mut self) {
        for records in self.entries.values_mut() {
            records.sort_by(|left, right| {
                (
                    &left.distribution,
                    &left.package_name,
                    &left.package_version,
                    &left.architecture,
                )
                    .cmp(&(
                        &right.distribution,
                        &right.package_name,
                        &right.package_version,
                        &right.architecture,
                    ))
            });
        }
    }
}

pub fn normalize_config_name(input: &str) -> String {
    let trimmed = input.trim().to_ascii_uppercase();
    if trimmed.starts_with("CONFIG_") {
        trimmed
    } else {
        format!("CONFIG_{trimmed}")
    }
}

pub fn parse_enabled_kernel_config(config_text: &str) -> BTreeMap<String, ConfigValue> {
    let mut entries = BTreeMap::new();

    for line in config_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("# CONFIG_") || line.starts_with("##") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_enabled_config_values_and_ignores_disabled_entries() {
        let parsed = parse_enabled_kernel_config(
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
        assert!(!parsed.contains_key("CONFIG_UNUSED"));
        assert!(!parsed.contains_key("NOT_A_CONFIG"));
    }

    #[test]
    fn builds_index_records_from_kernel_config_packages() {
        let package = KernelConfigPackage {
            distribution: "debian".to_string(),
            package_name: "linux-image-amd64".to_string(),
            package_version: "6.1.0-1".to_string(),
            architecture: "amd64".to_string(),
            source: Some("https://example.invalid/linux-image.deb".to_string()),
            config_text: "CONFIG_BPF=y\nCONFIG_EXT4_FS=m\n".to_string(),
        };

        let index = ConfigIndex::from_packages([package]);

        let bpf = index.entries.get("CONFIG_BPF").expect("CONFIG_BPF entry");
        assert_eq!(bpf.len(), 1);
        assert_eq!(bpf[0].distribution, "debian");
        assert_eq!(bpf[0].value, ConfigValue::BuiltIn);
    }

    #[test]
    fn normalizes_user_supplied_config_names() {
        assert_eq!(normalize_config_name("bpf"), "CONFIG_BPF");
        assert_eq!(normalize_config_name(" config_ext4_fs "), "CONFIG_EXT4_FS");
    }
}
