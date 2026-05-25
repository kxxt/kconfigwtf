use std::fs;

use assert_cmd::Command;
use kconfigwtf::index::{Architecture, Distribution, PackageIndex};
use kconfigwtf::indexer::KernelConfigPackage;
use serde_json::Value;

#[test]
fn migrate_command_rewrites_legacy_indexes_and_splits_by_release() {
    let temp = tempfile::tempdir().expect("tempdir");
    let package_dir = temp.path().join("debian/linux-image-amd64");
    fs::create_dir_all(&package_dir).expect("create package dir");
    let bookworm_config = (0..64)
        .map(|index| format!("CONFIG_BOOKWORM_{index}=y\n"))
        .collect::<String>();
    let trixie_config = (0..64)
        .map(|index| format!("CONFIG_TRIXIE_{index}=y\n"))
        .collect::<String>();

    let bookworm_entries = (0..64)
        .map(|index| {
            format!(
                r#"
            "CONFIG_BOOKWORM_{index}": [
              {{
                "kernel": "6.1.0-1/amd64",
                "value": "built_in"
              }}
            ]"#
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let trixie_entries = (0..64)
        .map(|index| {
            format!(
                r#"
            "CONFIG_TRIXIE_{index}": [
              {{
                "kernel": "6.6.0-1/amd64",
                "value": "built_in"
              }}
            ]"#
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
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
    let full_index =
        PackageIndex::from_packages([bookworm_package.clone(), trixie_package.clone()]);
    let bookworm_kernels = full_index
        .kernels
        .iter()
        .filter(|(_, kernel)| kernel.release == "bookworm")
        .map(|(kernel, _)| kernel.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let trixie_kernels = full_index
        .kernels
        .iter()
        .filter(|(_, kernel)| kernel.release == "trixie")
        .map(|(kernel, _)| kernel.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let bookworm_subset = PackageIndex {
        schema_version: full_index.schema_version,
        generated_at: full_index.generated_at,
        distribution: full_index.distribution.clone(),
        package_name: full_index.package_name.clone(),
        kernels: full_index
            .kernels
            .iter()
            .filter(|(kernel, _)| bookworm_kernels.contains(*kernel))
            .map(|(kernel, value)| (kernel.clone(), value.clone()))
            .collect(),
        entries: full_index
            .entries
            .iter()
            .map(|(name, occurrences)| {
                (
                    name.clone(),
                    occurrences
                        .iter()
                        .filter(|occurrence| bookworm_kernels.contains(&occurrence.kernel))
                        .cloned()
                        .collect(),
                )
            })
            .collect(),
    };
    let trixie_subset = PackageIndex {
        schema_version: full_index.schema_version,
        generated_at: full_index.generated_at,
        distribution: full_index.distribution.clone(),
        package_name: full_index.package_name.clone(),
        kernels: full_index
            .kernels
            .iter()
            .filter(|(kernel, _)| trixie_kernels.contains(*kernel))
            .map(|(kernel, value)| (kernel.clone(), value.clone()))
            .collect(),
        entries: full_index
            .entries
            .iter()
            .map(|(name, occurrences)| {
                (
                    name.clone(),
                    occurrences
                        .iter()
                        .filter(|occurrence| trixie_kernels.contains(&occurrence.kernel))
                        .cloned()
                        .collect(),
                )
            })
            .collect(),
    };
    let shard_max = serde_json::to_string_pretty(&bookworm_subset)
        .expect("serialize bookworm shard")
        .len()
        .max(
            serde_json::to_string_pretty(&trixie_subset)
                .expect("serialize trixie shard")
                .len(),
        );
    let full_len = serde_json::to_string_pretty(&full_index)
        .expect("serialize full index")
        .len();
    let max_index_bytes = shard_max + ((full_len - shard_max) / 2);

    fs::write(
        package_dir.join("index.json"),
        format!(
            r#"{{
          "schema_version": 5,
          "generated_at": "2026-01-01T00:00:00Z",
          "distribution": "debian",
          "package_name": "linux-image-amd64",
          "kernels": {{
            "6.1.0-1/amd64": {{
              "version": "6.1.0-1",
              "release": "bookworm",
              "architecture": "amd64",
              "config_path": "6.1.0-1/amd64/config"
            }},
            "6.6.0-1/amd64": {{
              "version": "6.6.0-1",
              "release": "trixie",
              "architecture": "amd64",
              "config_path": "6.6.0-1/amd64/config"
            }}
          }},
          "entries": {{
            {bookworm_entries},
            {trixie_entries},
            "CONFIG_UNUSED": [
              {{
                "kernel": "6.1.0-1/amd64",
                "value": "-"
              }},
              {{
                "kernel": "6.6.0-1/amd64",
                "value": "-"
              }}
            ]
          }}
        }}"#
        ),
    )
    .expect("write legacy package index");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "migrate",
            "--data-dir",
            temp.path().to_str().expect("temp path"),
        ])
        .args(["--max-index-bytes", &max_index_bytes.to_string()])
        .assert()
        .success();

    assert!(!package_dir.join("index.json").exists());
    assert!(package_dir.join("index_bookworm.json").exists());
    assert!(package_dir.join("index_trixie.json").exists());

    let migrated: Value = serde_json::from_str(
        &fs::read_to_string(package_dir.join("index_bookworm.json"))
            .expect("read migrated package index"),
    )
    .expect("parse migrated package index");

    assert_eq!(migrated["schema_version"], Value::from(6));
    assert!(migrated["releases"].is_array());
    assert!(migrated["architectures"].is_array());
    assert!(migrated["kernels"].is_array());
    assert!(migrated["entries"]["CONFIG_BOOKWORM_0"]["built_in"].is_array());
    assert!(migrated["entries"]["CONFIG_UNUSED"].is_object());
}
