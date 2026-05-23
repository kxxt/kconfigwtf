use std::fs;
use std::io::Write;

use assert_cmd::Command;
use flate2::Compression;
use flate2::write::GzEncoder;
use kconfigwtf::index::{
    Architecture, ConfigValue, Distribution, PackageIndex, write_packages_to_data_dir,
};
use kconfigwtf::indexer::KernelConfigPackage;
use liblzma::write::XzEncoder;
use predicates::prelude::*;
use rpm::{BuildConfig, CompressionType, FileOptions, PackageBuilder};
use tar::{Builder, Header};

#[derive(serde::Deserialize)]
struct TestManifest {
    indexes: Vec<String>,
    configs: Vec<String>,
}

#[test]
fn site_command_generates_static_site_from_data_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let data_dir = temp.path().join("data");
    let site_dir = temp.path().join("public");
    write_packages_to_data_dir(
        [KernelConfigPackage {
            distribution: Distribution::Debian,
            package_name: "linux-image-amd64".to_string(),
            package_version: "6.1.0-1".to_string(),
            architecture: Architecture::Amd64,
            source: None,
            config_text: "CONFIG_BPF=y\nCONFIG_EXT4_FS=m\n".to_string(),
        }],
        &data_dir,
    )
    .expect("write data");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "site",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
            "--output-dir",
            site_dir.to_str().expect("site dir"),
            "--title",
            "kconfigwtf test",
        ])
        .assert()
        .success();

    assert!(site_dir.join("index.html").exists());
    assert!(site_dir.join("app.js").exists());
    assert!(site_dir.join("styles.css").exists());
    assert!(site_dir.join("indexes.json").exists());
    let manifest: TestManifest =
        serde_json::from_str(&fs::read_to_string(site_dir.join("indexes.json")).expect("manifest"))
            .expect("parse manifest");
    assert_eq!(
        manifest.indexes,
        vec!["data/debian/linux-image-amd64/index.json"]
    );
    assert_eq!(manifest.configs, vec!["BPF", "EXT4_FS"]);
    assert!(
        site_dir
            .join("data/debian/linux-image-amd64/6.1.0-1/amd64/config")
            .exists()
    );
    let html = fs::read_to_string(site_dir.join("index.html")).expect("read html");
    assert!(html.contains("kconfigwtf test"));
    assert!(html.contains(r#"list="config-options""#));
    assert!(html.contains(r#"placeholder="BPF""#));
    assert!(html.contains(r#"<datalist id="config-options"></datalist>"#));
    assert!(html.contains("Versions / architectures"));
    assert!(site_dir.join("CONFIG_/BPF/index.html").exists());

    let bpf_page =
        fs::read_to_string(site_dir.join("CONFIG_/BPF/index.html")).expect("read bpf page");
    assert!(bpf_page.contains("CONFIG_BPF"));
    assert!(bpf_page.contains(r#"rowspan="1""#));
    assert!(bpf_page.contains("kernel-tag"));
    assert!(bpf_page.contains("arch-button"));
    assert!(
        bpf_page.contains(
            r#"data-config-url="../../data/debian/linux-image-amd64/6.1.0-1/amd64/config""#
        )
    );

    let app = fs::read_to_string(site_dir.join("app.js")).expect("read app js");
    assert!(!app.contains("collectConfigNames"));
    assert!(app.contains("manifest.configs"));
    assert!(app.contains("arch-button"));
    assert!(app.contains("window.location.href"));
    assert!(app.contains(r#"input.addEventListener("input""#));
    assert!(app.contains(r#"input.addEventListener("change""#));
    assert!(app.contains("navigateIfExactConfig"));
    assert!(app.contains("shouldNavigateFromInputEvent(event)"));
    assert!(app.contains(r#"inputType === "insertText""#));
    assert!(app.contains(r#"inputType === "insertReplacementText""#));
    assert!(app.contains("navigateToConfig(configName)"));
    assert!(app.contains("CONFIG_"));
}

#[test]
fn debian_index_command_indexes_local_packages_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let deb_root = temp.path().join("mirror");
    let deb_path = deb_root.join("pool/main/l/linux/linux-image-test.deb");
    let packages_path = temp.path().join("Packages");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(deb_path.parent().expect("deb parent")).expect("create pool");
    fs::write(
        &deb_path,
        minimal_deb_with_config("CONFIG_BPF=y\nCONFIG_EXT4_FS=m\n# CONFIG_UNUSED is not set\n"),
    )
    .expect("write deb");
    fs::write(
        &packages_path,
        "Package: linux-image-6.1.0-1-amd64\nVersion: 6.1.4-1\nFilename: pool/main/l/linux/linux-image-test.deb\n",
    )
    .expect("write packages");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "debian",
            "--packages-file",
            packages_path.to_str().expect("packages path"),
            "--deb-root",
            deb_root.to_str().expect("deb root"),
            "--arch",
            "amd64",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    let config_path = data_dir.join("debian/linux-image-<VERSION>-<ARCH>/6.1.4-1/amd64/config");
    assert!(config_path.exists());
    assert!(
        fs::read_to_string(&config_path)
            .expect("read raw config")
            .contains("CONFIG_BPF=y")
    );

    let index_path = data_dir.join("debian/linux-image-<VERSION>-<ARCH>/index.json");
    let index: PackageIndex =
        serde_json::from_str(&fs::read_to_string(&index_path).expect("read output"))
            .expect("parse package index");
    let bpf = index.entries.get("CONFIG_BPF").expect("CONFIG_BPF");

    assert_eq!(bpf.len(), 1);
    assert_eq!(index.distribution, Distribution::Debian);
    assert_eq!(index.package_name, "linux-image-<VERSION>-<ARCH>");
    assert_eq!(
        index.kernels["6.1.4-1/amd64"].architecture,
        Architecture::Amd64
    );
    assert_eq!(bpf[0].value, ConfigValue::BuiltIn);
    assert_eq!(
        index.entries.get("CONFIG_UNUSED").expect("CONFIG_UNUSED")[0].value,
        ConfigValue::Missing
    );
}

#[test]
fn debian_index_command_requires_deb_root_for_local_packages_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let packages_path = temp.path().join("Packages");
    fs::write(&packages_path, "").expect("write packages");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "debian",
            "--packages-file",
            packages_path.to_str().expect("packages path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--deb-root is required"));
}

#[test]
fn android_index_command_requires_artifact_root_for_local_release_builds_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let release_builds_path = temp.path().join("release-builds.json");
    fs::write(&release_builds_path, "{}").expect("write release builds");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "android",
            "--release-builds-file",
            release_builds_path.to_str().expect("release builds path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--artifact-root is required"));
}

#[test]
fn android_index_command_indexes_local_release_builds_metadata() {
    let temp = tempfile::tempdir().expect("tempdir");
    let release_builds_path = temp.path().join("release-builds.html");
    let artifact_root = temp.path().join("artifacts");
    let config_path = artifact_root
        .join("13586339")
        .join("kernel_aarch64")
        .join("kernel_aarch64_dot_config");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(config_path.parent().expect("config parent")).expect("create artifacts");
    fs::write(
        &release_builds_path,
        r#"<devsite-code><pre><code>
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
    .expect("write release builds");
    fs::write(&config_path, "CONFIG_BPF=y\n# CONFIG_UNUSED is not set\n")
        .expect("write android config");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "android",
            "--branch",
            "android16-6.12",
            "--release-builds-file",
            release_builds_path.to_str().expect("release builds path"),
            "--artifact-root",
            artifact_root.to_str().expect("artifact root"),
            "--max-builds",
            "1",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    let output_config =
        data_dir.join("android/android16-6.12/android16-6.12-2025-06_r1/arm64/config");
    assert!(output_config.exists());
    assert!(
        fs::read_to_string(&output_config)
            .expect("read android config")
            .contains("CONFIG_BPF=y")
    );

    let index_path = data_dir.join("android/android16-6.12/index.json");
    let index: PackageIndex =
        serde_json::from_str(&fs::read_to_string(&index_path).expect("read android index"))
            .expect("parse android index");
    assert_eq!(index.distribution, Distribution::Android);
    assert_eq!(index.package_name, "android16-6.12");
    assert_eq!(
        index.kernels["android16-6.12-2025-06_r1/arm64"].architecture,
        Architecture::Arm64
    );
    assert_eq!(
        index.entries.get("CONFIG_UNUSED").expect("CONFIG_UNUSED")[0].value,
        ConfigValue::Missing
    );
}

#[test]
fn android_index_command_discovers_branches_from_local_overview() {
    let temp = tempfile::tempdir().expect("tempdir");
    let discovery_path = temp.path().join("overview.html");
    let release_builds_root = temp.path().join("release-builds");
    let artifact_root = temp.path().join("artifacts");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(&release_builds_root).expect("create release builds root");
    fs::write(
        &discovery_path,
        r#"
<a href="/docs/core/architecture/kernel/gki-android16-6_12-release-builds">android16-6.12</a>
<a href="/docs/core/architecture/kernel/gki-android15-6_6-release-builds">android15-6.6</a>
"#,
    )
    .expect("write discovery page");
    write_android_release_builds(
        &release_builds_root.join("gki-android16-6_12-release-builds.html"),
        "android16-6.12",
        "android16-6.12-2025-06",
        "android16-6.12-2025-06_r1",
        "13586339",
    );
    write_android_release_builds(
        &release_builds_root.join("gki-android15-6_6-release-builds.html"),
        "android15-6.6",
        "android15-6.6-2025-04",
        "android15-6.6-2025-04_r1",
        "12445566",
    );
    write_android_config(&artifact_root, "13586339", "CONFIG_ANDROID16=y\n");
    write_android_config(&artifact_root, "12445566", "CONFIG_ANDROID15=y\n");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "android",
            "--discovery-file",
            discovery_path.to_str().expect("discovery path"),
            "--release-builds-root",
            release_builds_root.to_str().expect("release builds root"),
            "--artifact-root",
            artifact_root.to_str().expect("artifact root"),
            "--max-builds",
            "1",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    assert!(
        data_dir
            .join("android/android16-6.12/android16-6.12-2025-06_r1/arm64/config")
            .exists()
    );
    assert!(
        data_dir
            .join("android/android15-6.6/android15-6.6-2025-04_r1/arm64/config")
            .exists()
    );
}

#[test]
fn ubuntu_index_command_indexes_local_packages_file() {
    apt_index_command_indexes_local_packages_file(AptCliCase {
        command: "ubuntu",
        distribution: Distribution::Ubuntu,
        package_name: "linux-modules-6.14.0-29-generic",
        package_version: "6.14.0-29.29~24.04.1",
        expected_config_path: "ubuntu/linux-image-<VERSION>-generic/6.14.0-29.29~24.04.1/amd64/config",
        expected_index_path: "ubuntu/linux-image-<VERSION>-generic/index.json",
        expected_index_package_name: "linux-image-<VERSION>-generic",
        extra_packages: &[],
    });
}

fn write_android_release_builds(
    path: &std::path::Path,
    name: &str,
    branch: &str,
    tag: &str,
    build_id: &str,
) {
    fs::write(
        path,
        format!(
            r#"<devsite-code><pre><code>
{{
  "name": "{name}",
  "branches": [
    {{
      "name": "{branch}",
      "kernel_version": "6.12.23",
      "releases": [
        {{
          "tag": "{tag}",
          "date": "2025-06-12",
          "sha1": "2d954fcf3d1b73a41d0fa498324da357ec96cbdf",
          "kernel_bid": "{build_id}"
        }}
      ]
    }}
  ]
}}
</code></pre></devsite-code>"#
        ),
    )
    .expect("write android release builds");
}

fn write_android_config(artifact_root: &std::path::Path, build_id: &str, config: &str) {
    let config_path = artifact_root
        .join(build_id)
        .join("kernel_aarch64")
        .join("kernel_aarch64_dot_config");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("create artifacts");
    fs::write(config_path, config).expect("write android config");
}

#[test]
fn kali_index_command_indexes_local_packages_file() {
    apt_index_command_indexes_local_packages_file(AptCliCase {
        command: "kali",
        distribution: Distribution::Kali,
        package_name: "linux-base-6.19.14+kali-amd64",
        package_version: "6.19.14-1+kali1",
        expected_config_path: "kali/linux-image-<VERSION>-<ARCH>/6.19.14-1+kali1/amd64/config",
        expected_index_path: "kali/linux-image-<VERSION>-<ARCH>/index.json",
        expected_index_package_name: "linux-image-<VERSION>-<ARCH>",
        extra_packages: &[],
    });
}

#[test]
fn proxmox_index_command_indexes_local_packages_file() {
    apt_index_command_indexes_local_packages_file(AptCliCase {
        command: "proxmox",
        distribution: Distribution::Proxmox,
        package_name: "proxmox-kernel-6.11.0-1-pve",
        package_version: "6.11.0-1",
        expected_config_path: "proxmox/proxmox-kernel-<VERSION>-pve/6.11.0-1/amd64/config",
        expected_index_path: "proxmox/proxmox-kernel-<VERSION>-pve/index.json",
        expected_index_package_name: "proxmox-kernel-<VERSION>-pve",
        extra_packages: &[(
            "proxmox-kernel-6.11.0-1-pve-signed",
            "6.11.0-1",
            "pool/p/proxmox-signed.deb",
        )],
    });
}

#[test]
fn deepin_index_command_indexes_local_packages_file() {
    apt_index_command_indexes_local_packages_file(AptCliCase {
        command: "deepin",
        distribution: Distribution::Deepin,
        package_name: "linux-image-deepin-amd64",
        package_version: "6.1.0-18",
        expected_config_path: "deepin/linux-image-deepin-<ARCH>/6.1.0-18/amd64/config",
        expected_index_path: "deepin/linux-image-deepin-<ARCH>/index.json",
        expected_index_package_name: "linux-image-deepin-<ARCH>",
        extra_packages: &[],
    });
}

#[test]
fn kylin_index_command_indexes_local_packages_file() {
    apt_index_command_indexes_local_packages_file(AptCliCase {
        command: "kylin",
        distribution: Distribution::Kylin,
        package_name: "linux-image-5.10.0-generic",
        package_version: "5.10.0-1",
        expected_config_path: "kylin/linux-image-<VERSION>-generic/5.10.0-1/amd64/config",
        expected_index_path: "kylin/linux-image-<VERSION>-generic/index.json",
        expected_index_package_name: "linux-image-<VERSION>-generic",
        extra_packages: &[],
    });
}

#[test]
fn aosc_index_command_indexes_local_packages_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let deb_root = temp.path().join("mirror");
    let deb_path = deb_root.join("pool/main/l/linux/linux-kernel-6.14.0.deb");
    let packages_path = temp.path().join("Packages.xz");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(deb_path.parent().expect("deb parent")).expect("create pool");
    fs::write(
        &deb_path,
        minimal_deb_with_config("CONFIG_BPF=y\n# CONFIG_UNUSED is not set\n"),
    )
    .expect("write deb");

    let packages_content = "Package: linux-kernel-6.14.0\nVersion: 6.14.0-1\nArchitecture: amd64\nFilename: pool/main/l/linux/linux-kernel-6.14.0.deb\n\n";
    let mut encoder = XzEncoder::new(Vec::new(), 6);
    encoder
        .write_all(packages_content.as_bytes())
        .expect("write xz");
    let compressed_packages = encoder.finish().expect("finish xz");
    fs::write(&packages_path, compressed_packages).expect("write packages.xz");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "aosc",
            "--packages-file",
            packages_path.to_str().expect("packages path"),
            "--deb-root",
            deb_root.to_str().expect("deb root"),
            "--arch",
            "amd64",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    let config_path = data_dir.join("aoscos/linux-kernel-<VERSION>/6.14.0-1/amd64/config");
    assert!(config_path.exists());
    assert!(
        fs::read_to_string(&config_path)
            .expect("read raw config")
            .contains("CONFIG_BPF=y")
    );

    let index_path = data_dir.join("aoscos/linux-kernel-<VERSION>/index.json");
    let index: PackageIndex =
        serde_json::from_str(&fs::read_to_string(&index_path).expect("read output"))
            .expect("parse package index");
    assert_eq!(index.distribution, Distribution::AoscOS);
    assert_eq!(index.package_name, "linux-kernel-<VERSION>");
    assert_eq!(
        index.entries.get("CONFIG_UNUSED").expect("CONFIG_UNUSED")[0].value,
        ConfigValue::Missing
    );
}

#[test]
fn aosc_index_command_extracts_embedded_config_from_kernel_image() {
    let temp = tempfile::tempdir().expect("tempdir");
    let deb_root = temp.path().join("mirror");
    let deb_path = deb_root.join("pool/main/l/linux/linux-kernel-6.18.27.deb");
    let packages_path = temp.path().join("Packages.xz");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(deb_path.parent().expect("deb parent")).expect("create pool");
    fs::write(
        &deb_path,
        minimal_deb_with_file(
            "./boot/vmlinuz-6.18.27-aosc-main",
            &fake_ikconfig_image("CONFIG_AOSC=y\n# CONFIG_UNUSED is not set\n"),
        ),
    )
    .expect("write deb");

    let packages_content = "Package: linux-kernel-6.18.27\nVersion: 6.18.27-1\nArchitecture: amd64\nFilename: pool/main/l/linux/linux-kernel-6.18.27.deb\n\n";
    let mut encoder = XzEncoder::new(Vec::new(), 6);
    encoder
        .write_all(packages_content.as_bytes())
        .expect("write xz");
    let compressed_packages = encoder.finish().expect("finish xz");
    fs::write(&packages_path, compressed_packages).expect("write packages.xz");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "aosc",
            "--packages-file",
            packages_path.to_str().expect("packages path"),
            "--deb-root",
            deb_root.to_str().expect("deb root"),
            "--arch",
            "amd64",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    let config_path = data_dir.join("aoscos/linux-kernel-<VERSION>/6.18.27-1/amd64/config");
    assert!(config_path.exists());
    assert!(
        fs::read_to_string(&config_path)
            .expect("read raw config")
            .contains("CONFIG_AOSC=y")
    );
}

#[test]
fn aosc_index_command_normalizes_rc_kernel_package_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    let deb_root = temp.path().join("mirror");
    let packages_path = temp.path().join("Packages.xz");
    let data_dir = temp.path().join("data");
    let rc_deb = deb_root.join("pool/main/l/linux/linux-kernel-rc-6.18.0.deb");
    let vanillarc_deb = deb_root.join("pool/main/l/linux/linux-kernel-vanillarc-7.0.0.deb");

    fs::create_dir_all(rc_deb.parent().expect("deb parent")).expect("create pool");
    fs::write(
        &rc_deb,
        minimal_deb_with_file(
            "./boot/vmlinuz-6.18.0-aosc-main",
            &fake_ikconfig_image("CONFIG_AOSC_RC=y\n"),
        ),
    )
    .expect("write rc deb");
    fs::write(
        &vanillarc_deb,
        minimal_deb_with_file(
            "./boot/vmlinuz-7.0.0-aosc-main",
            &fake_ikconfig_image("CONFIG_AOSC_VANILLARC=y\n"),
        ),
    )
    .expect("write vanillarc deb");

    let packages_content = "\
Package: linux-kernel-rc-6.18.0
Version: 6.18.0-0.7
Architecture: amd64
Filename: pool/main/l/linux/linux-kernel-rc-6.18.0.deb

Package: linux-kernel-vanillarc-7.0.0
Version: 7.0.0-0.2
Architecture: amd64
Filename: pool/main/l/linux/linux-kernel-vanillarc-7.0.0.deb

";
    let mut encoder = XzEncoder::new(Vec::new(), 6);
    encoder
        .write_all(packages_content.as_bytes())
        .expect("write xz");
    let compressed_packages = encoder.finish().expect("finish xz");
    fs::write(&packages_path, compressed_packages).expect("write packages.xz");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "aosc",
            "--packages-file",
            packages_path.to_str().expect("packages path"),
            "--deb-root",
            deb_root.to_str().expect("deb root"),
            "--arch",
            "amd64",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    assert!(
        data_dir
            .join("aoscos/linux-kernel-rc-<VERSION>/6.18.0-0.7/amd64/config")
            .exists()
    );
    assert!(
        data_dir
            .join("aoscos/linux-kernel-vanillarc-<VERSION>/7.0.0-0.2/amd64/config")
            .exists()
    );
}

#[test]
fn arch_index_command_requires_package_root_for_local_db_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("core.db");
    fs::write(&db_path, "").expect("write db");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "arch",
            "--distribution",
            "cachyos",
            "--db-file",
            db_path.to_str().expect("db path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--package-root is required"));
}

struct AptCliCase<'a> {
    command: &'a str,
    distribution: Distribution,
    package_name: &'a str,
    package_version: &'a str,
    expected_config_path: &'a str,
    expected_index_path: &'a str,
    expected_index_package_name: &'a str,
    extra_packages: &'a [(&'a str, &'a str, &'a str)],
}

fn apt_index_command_indexes_local_packages_file(case: AptCliCase<'_>) {
    let temp = tempfile::tempdir().expect("tempdir");
    let deb_root = temp.path().join("mirror");
    let deb_path = deb_root.join("pool/main/l/linux/kernel.deb");
    let packages_path = temp.path().join("Packages");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(deb_path.parent().expect("deb parent")).expect("create pool");
    fs::write(
        &deb_path,
        minimal_deb_with_config("CONFIG_BPF=y\n# CONFIG_UNUSED is not set\n"),
    )
    .expect("write deb");

    let mut packages = format!(
        "Package: {}\nVersion: {}\nArchitecture: amd64\nFilename: pool/main/l/linux/kernel.deb\n\n",
        case.package_name, case.package_version
    );
    for (name, version, filename) in case.extra_packages {
        packages.push_str(&format!(
            "Package: {name}\nVersion: {version}\nArchitecture: amd64\nFilename: {filename}\n\n"
        ));
    }
    fs::write(&packages_path, packages).expect("write packages");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            case.command,
            "--packages-file",
            packages_path.to_str().expect("packages path"),
            "--deb-root",
            deb_root.to_str().expect("deb root"),
            "--arch",
            "amd64",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    let config_path = data_dir.join(case.expected_config_path);
    assert!(config_path.exists());
    assert!(
        fs::read_to_string(&config_path)
            .expect("read raw config")
            .contains("CONFIG_BPF=y")
    );

    let index_path = data_dir.join(case.expected_index_path);
    let index: PackageIndex =
        serde_json::from_str(&fs::read_to_string(&index_path).expect("read output"))
            .expect("parse package index");
    assert_eq!(index.distribution, case.distribution);
    assert_eq!(index.package_name, case.expected_index_package_name);
    assert_eq!(
        index.entries.get("CONFIG_UNUSED").expect("CONFIG_UNUSED")[0].value,
        ConfigValue::Missing
    );
}

#[test]
fn arch_index_command_indexes_local_sync_database() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    let package_path = repo_root.join("linux-headers-6.12.1.arch1-1-x86_64.pkg.tar.zst");
    let db_path = temp.path().join("core.db");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(&repo_root).expect("create repo root");
    fs::write(
        &package_path,
        minimal_arch_package_with_config("CONFIG_BPF=y\n# CONFIG_UNUSED is not set\n"),
    )
    .expect("write arch package");
    fs::write(
        &db_path,
        gzip_raw_bytes(&tar_with_file(
            "linux-6.12.1.arch1-1/desc",
            br#"%FILENAME%
linux-headers-6.12.1.arch1-1-x86_64.pkg.tar.zst

%NAME%
linux-headers

%VERSION%
6.12.1.arch1-1

%ARCH%
x86_64
"#,
        )),
    )
    .expect("write sync database");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "arch",
            "--distribution",
            "archlinux",
            "--db-file",
            db_path.to_str().expect("db path"),
            "--package-root",
            repo_root.to_str().expect("repo root"),
            "--arch",
            "x86_64",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    let config_path = data_dir.join("archlinux/linux/6.12.1.arch1-1/amd64/config");
    assert!(config_path.exists());
    assert!(
        fs::read_to_string(&config_path)
            .expect("read arch config")
            .contains("CONFIG_BPF=y")
    );

    let index_path = data_dir.join("archlinux/linux/index.json");
    let index: PackageIndex =
        serde_json::from_str(&fs::read_to_string(&index_path).expect("read arch index"))
            .expect("parse arch index");
    assert_eq!(index.distribution, Distribution::ArchLinux);
    assert_eq!(index.package_name, "linux");
    assert_eq!(
        index.kernels["6.12.1.arch1-1/amd64"].architecture,
        Architecture::Amd64
    );
    assert_eq!(
        index.entries.get("CONFIG_UNUSED").expect("CONFIG_UNUSED")[0].value,
        ConfigValue::Missing
    );
}

#[test]
fn eweos_index_command_indexes_local_sync_database() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    let package_path = repo_root.join("linux-devel-7.0.9-1-x86_64.pkg.tar.zst");
    let db_path = temp.path().join("main.db");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(&repo_root).expect("create repo root");
    fs::write(
        &package_path,
        zstd::encode_all(
            tar_with_file(
                "usr/src/linux/.config",
                b"CONFIG_BPF=y\n# CONFIG_UNUSED is not set\n",
            )
            .as_slice(),
            0,
        )
        .expect("write zstd"),
    )
    .expect("write eweOS package");
    fs::write(
        &db_path,
        gzip_raw_bytes(&tar_with_file(
            "linux-7.0.9-1/desc",
            br#"%FILENAME%
linux-devel-7.0.9-1-x86_64.pkg.tar.zst

%NAME%
linux-devel

%VERSION%
7.0.9-1

%ARCH%
x86_64
"#,
        )),
    )
    .expect("write sync database");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "eweos",
            "--db-file",
            db_path.to_str().expect("db path"),
            "--package-root",
            repo_root.to_str().expect("repo root"),
            "--arch",
            "x86_64",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    let config_path = data_dir.join("eweos/linux/7.0.9-1/amd64/config");
    assert!(config_path.exists());
    assert!(
        fs::read_to_string(&config_path)
            .expect("read eweOS config")
            .contains("CONFIG_BPF=y")
    );

    let index_path = data_dir.join("eweos/linux/index.json");
    let index: PackageIndex =
        serde_json::from_str(&fs::read_to_string(&index_path).expect("read eweOS index"))
            .expect("parse eweOS index");
    assert_eq!(index.distribution, Distribution::EweOS);
    assert_eq!(index.package_name, "linux");
}

#[test]
fn alpine_index_command_requires_apk_root_for_local_apkindex_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let apkindex_path = temp.path().join("APKINDEX.tar.gz");
    fs::write(&apkindex_path, "").expect("write APKINDEX");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "alpine",
            "--apkindex-file",
            apkindex_path.to_str().expect("APKINDEX path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--apk-root is required"));
}

#[test]
fn alpine_index_command_indexes_local_apkindex() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    let package_path = repo_root.join("linux-lts-6.18.32-r0.apk");
    let apkindex_path = temp.path().join("APKINDEX.tar.gz");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(&repo_root).expect("create repo root");
    fs::write(
        &package_path,
        gzip_raw_bytes(&tar_with_file(
            "boot/config-6.18.32-0-lts",
            b"CONFIG_BPF=y\n# CONFIG_UNUSED is not set\n",
        )),
    )
    .expect("write apk package");
    fs::write(
        &apkindex_path,
        gzip_raw_bytes(&tar_with_file(
            "APKINDEX",
            br#"P:linux-lts
V:6.18.32-r0
A:x86_64

P:linux-lts-dev
V:6.18.32-r0
A:x86_64
"#,
        )),
    )
    .expect("write APKINDEX");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "alpine",
            "--apkindex-file",
            apkindex_path.to_str().expect("APKINDEX path"),
            "--apk-root",
            repo_root.to_str().expect("repo root"),
            "--arch",
            "x86_64",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    let config_path = data_dir.join("alpine/linux-lts/6.18.32-r0/amd64/config");
    assert!(config_path.exists());
    assert!(
        fs::read_to_string(&config_path)
            .expect("read Alpine config")
            .contains("CONFIG_BPF=y")
    );

    let index_path = data_dir.join("alpine/linux-lts/index.json");
    let index: PackageIndex =
        serde_json::from_str(&fs::read_to_string(&index_path).expect("read Alpine index"))
            .expect("parse Alpine index");
    assert_eq!(index.distribution, Distribution::Alpine);
    assert_eq!(index.package_name, "linux-lts");
    assert_eq!(
        index.entries.get("CONFIG_UNUSED").expect("CONFIG_UNUSED")[0].value,
        ConfigValue::Missing
    );
}

#[test]
fn fedora_index_command_requires_rpm_root_for_local_repomd_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repomd_path = temp.path().join("repomd.xml");
    fs::write(&repomd_path, "").expect("write repomd");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "fedora",
            "--repomd-file",
            repomd_path.to_str().expect("repomd path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--rpm-root is required"));
}

#[test]
fn fedora_index_command_indexes_local_repo_metadata() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    let repodata = repo_root.join("repodata");
    let packages_dir = repo_root.join("Packages/k");
    let rpm_path = packages_dir.join("kernel-core-test.rpm");
    let primary_path = repodata.join("primary.xml.gz");
    let repomd_path = repodata.join("repomd.xml");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(&packages_dir).expect("create packages dir");
    fs::create_dir_all(&repodata).expect("create repodata dir");
    fs::write(&rpm_path, minimal_rpm_with_config("CONFIG_BPF=y\n")).expect("write rpm");
    fs::write(
        &primary_path,
        gzip_bytes(
            r#"<metadata>
  <package type="rpm">
    <name>kernel-core</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="6.12.0" rel="1.fc99"/>
    <location href="Packages/k/kernel-core-test.rpm"/>
  </package>
</metadata>"#,
        ),
    )
    .expect("write primary");
    fs::write(
        &repomd_path,
        r#"<repomd>
  <data type="primary"><location href="repodata/primary.xml.gz"/></data>
</repomd>"#,
    )
    .expect("write repomd");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "fedora",
            "--repomd-file",
            repomd_path.to_str().expect("repomd path"),
            "--rpm-root",
            repo_root.to_str().expect("repo root"),
            "--arch",
            "x86_64",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    let config_path = data_dir.join("fedora/kernel-core/0:6.12.0-1.fc99/amd64/config");
    assert!(config_path.exists());
    assert!(
        fs::read_to_string(&config_path)
            .expect("read fedora config")
            .contains("CONFIG_BPF=y")
    );

    let index_path = data_dir.join("fedora/kernel-core/index.json");
    let index: PackageIndex =
        serde_json::from_str(&fs::read_to_string(&index_path).expect("read fedora index"))
            .expect("parse fedora index");
    assert_eq!(index.distribution, Distribution::Fedora);
    assert_eq!(index.package_name, "kernel-core");
    assert_eq!(
        index.kernels["0:6.12.0-1.fc99/amd64"].architecture,
        Architecture::Amd64
    );
}

#[test]
fn rhel_index_command_indexes_local_repo_metadata() {
    rpm_index_command_indexes_local_repo_metadata(RpmCliCase {
        command: "rhel",
        release: None,
        distribution: Distribution::Rhel,
        package_name: "kernel-core",
        expected_config_path: "rhel/kernel-core/0:6.12.0-1.fc99/amd64/config",
        expected_index_path: "rhel/kernel-core/index.json",
    });
}

#[test]
fn centos_index_command_indexes_local_repo_metadata() {
    rpm_index_command_indexes_local_repo_metadata(RpmCliCase {
        command: "centos",
        release: None,
        distribution: Distribution::CentOS,
        package_name: "kernel-core",
        expected_config_path: "centos/kernel-core/0:6.12.0-1.fc99/amd64/config",
        expected_index_path: "centos/kernel-core/index.json",
    });
}

#[test]
fn centos_6_index_command_defaults_to_kernel_package() {
    rpm_index_command_indexes_local_repo_metadata(RpmCliCase {
        command: "centos",
        release: Some("6"),
        distribution: Distribution::CentOS,
        package_name: "kernel",
        expected_config_path: "centos/kernel/0:6.12.0-1.fc99/amd64/config",
        expected_index_path: "centos/kernel/index.json",
    });
}

#[test]
fn almalinux_index_command_indexes_local_repo_metadata() {
    rpm_index_command_indexes_local_repo_metadata(RpmCliCase {
        command: "almalinux",
        release: None,
        distribution: Distribution::AlmaLinux,
        package_name: "kernel-core",
        expected_config_path: "almalinux/kernel-core/0:6.12.0-1.fc99/amd64/config",
        expected_index_path: "almalinux/kernel-core/index.json",
    });
}

#[test]
fn rocky_index_command_indexes_local_repo_metadata() {
    rpm_index_command_indexes_local_repo_metadata(RpmCliCase {
        command: "rocky",
        release: None,
        distribution: Distribution::Rocky,
        package_name: "kernel-core",
        expected_config_path: "rocky/kernel-core/0:6.12.0-1.fc99/amd64/config",
        expected_index_path: "rocky/kernel-core/index.json",
    });
}

#[test]
fn openeuler_index_command_indexes_local_repo_metadata() {
    rpm_index_command_indexes_local_repo_metadata(RpmCliCase {
        command: "openeuler",
        release: None,
        distribution: Distribution::OpenEuler,
        package_name: "kernel",
        expected_config_path: "openeuler/kernel/0:6.12.0-1.fc99/amd64/config",
        expected_index_path: "openeuler/kernel/index.json",
    });
}

#[test]
fn openanolis_index_command_indexes_local_repo_metadata() {
    rpm_index_command_indexes_local_repo_metadata(RpmCliCase {
        command: "openanolis",
        release: None,
        distribution: Distribution::OpenAnolis,
        package_name: "kernel",
        expected_config_path: "openanolis/kernel/0:6.12.0-1.fc99/amd64/config",
        expected_index_path: "openanolis/kernel/index.json",
    });
}

#[test]
fn opensuse_index_command_indexes_local_repo_metadata() {
    rpm_index_command_indexes_local_repo_metadata(RpmCliCase {
        command: "opensuse",
        release: None,
        distribution: Distribution::OpenSUSE,
        package_name: "kernel-default",
        expected_config_path: "opensuse/kernel-default/0:6.12.0-1.fc99/amd64/config",
        expected_index_path: "opensuse/kernel-default/index.json",
    });
}

#[test]
fn opensuse_index_command_indexes_kernel_vanilla_by_default() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    let repodata = repo_root.join("repodata");
    let packages_dir = repo_root.join("x86_64");
    let default_rpm_path = packages_dir.join("kernel-default-test.rpm");
    let vanilla_rpm_path = packages_dir.join("kernel-vanilla-test.rpm");
    let primary_path = repodata.join("primary.xml.gz");
    let repomd_path = repodata.join("repomd.xml");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(&packages_dir).expect("create packages dir");
    fs::create_dir_all(&repodata).expect("create repodata dir");
    fs::write(
        &default_rpm_path,
        minimal_rpm_with_config("CONFIG_DEFAULT=y\n"),
    )
    .expect("write default rpm");
    fs::write(
        &vanilla_rpm_path,
        minimal_rpm_with_config("CONFIG_VANILLA=y\n"),
    )
    .expect("write vanilla rpm");
    fs::write(
        &primary_path,
        gzip_bytes(
            r#"<metadata>
  <package type="rpm">
    <name>kernel-default</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="7.0.9" rel="1.1"/>
    <location href="x86_64/kernel-default-test.rpm"/>
  </package>
  <package type="rpm">
    <name>kernel-vanilla</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="7.0.9" rel="1.1"/>
    <location href="x86_64/kernel-vanilla-test.rpm"/>
  </package>
</metadata>"#,
        ),
    )
    .expect("write primary");
    fs::write(
        &repomd_path,
        r#"<repomd>
  <data type="primary"><location href="repodata/primary.xml.gz"/></data>
</repomd>"#,
    )
    .expect("write repomd");

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args([
            "index",
            "opensuse",
            "--repomd-file",
            repomd_path.to_str().expect("repomd path"),
            "--rpm-root",
            repo_root.to_str().expect("repo root"),
            "--arch",
            "x86_64",
            "--data-dir",
            data_dir.to_str().expect("data dir"),
        ])
        .assert()
        .success();

    assert!(
        data_dir
            .join("opensuse/kernel-default/0:7.0.9-1.1/amd64/config")
            .exists()
    );
    assert!(
        data_dir
            .join("opensuse/kernel-vanilla/0:7.0.9-1.1/amd64/config")
            .exists()
    );
}

struct RpmCliCase<'a> {
    command: &'a str,
    release: Option<&'a str>,
    distribution: Distribution,
    package_name: &'a str,
    expected_config_path: &'a str,
    expected_index_path: &'a str,
}

fn rpm_index_command_indexes_local_repo_metadata(case: RpmCliCase<'_>) {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    let repodata = repo_root.join("repodata");
    let packages_dir = repo_root.join("Packages/k");
    let rpm_path = packages_dir.join("kernel-test.rpm");
    let primary_path = repodata.join("primary.xml.gz");
    let repomd_path = repodata.join("repomd.xml");
    let data_dir = temp.path().join("data");

    fs::create_dir_all(&packages_dir).expect("create packages dir");
    fs::create_dir_all(&repodata).expect("create repodata dir");
    fs::write(&rpm_path, minimal_rpm_with_config("CONFIG_BPF=y\n")).expect("write rpm");
    fs::write(
        &primary_path,
        gzip_bytes(&format!(
            r#"<metadata>
  <package type="rpm">
    <name>{}</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="6.12.0" rel="1.fc99"/>
    <location href="Packages/k/kernel-test.rpm"/>
  </package>
</metadata>"#,
            case.package_name
        )),
    )
    .expect("write primary");
    fs::write(
        &repomd_path,
        r#"<repomd>
  <data type="primary"><location href="repodata/primary.xml.gz"/></data>
</repomd>"#,
    )
    .expect("write repomd");

    let mut args = vec![
        "index".to_string(),
        case.command.to_string(),
        "--repomd-file".to_string(),
        repomd_path.to_str().expect("repomd path").to_string(),
        "--rpm-root".to_string(),
        repo_root.to_str().expect("repo root").to_string(),
        "--arch".to_string(),
        "x86_64".to_string(),
        "--data-dir".to_string(),
        data_dir.to_str().expect("data dir").to_string(),
    ];
    if let Some(release) = case.release {
        args.push("--release".to_string());
        args.push(release.to_string());
    }

    Command::cargo_bin("kconfigwtf")
        .expect("binary")
        .args(args)
        .assert()
        .success();

    let config_path = data_dir.join(case.expected_config_path);
    assert!(config_path.exists());
    assert!(
        fs::read_to_string(&config_path)
            .expect("read rpm config")
            .contains("CONFIG_BPF=y")
    );

    let index_path = data_dir.join(case.expected_index_path);
    let index: PackageIndex =
        serde_json::from_str(&fs::read_to_string(&index_path).expect("read rpm index"))
            .expect("parse rpm index");
    assert_eq!(index.distribution, case.distribution);
    assert_eq!(index.package_name, case.package_name);
    assert_eq!(
        index.kernels["0:6.12.0-1.fc99/amd64"].architecture,
        Architecture::Amd64
    );
}

fn minimal_deb_with_config(config: &str) -> Vec<u8> {
    minimal_deb_with_file("./boot/config-6.1.0-1-amd64", config.as_bytes())
}

fn minimal_deb_with_file(path: &str, contents: &[u8]) -> Vec<u8> {
    let mut tarball = Vec::new();
    {
        let mut builder = Builder::new(&mut tarball);
        let mut header = Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, path, contents)
            .expect("append file");
        builder.finish().expect("finish tar");
    }

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tarball).expect("write gzip");
    let data_tar_gz = gz.finish().expect("finish gzip");

    let mut deb = b"!<arch>\n".to_vec();
    append_ar_member(&mut deb, "debian-binary", b"2.0\n");
    append_ar_member(&mut deb, "control.tar.gz", &[]);
    append_ar_member(&mut deb, "data.tar.gz", &data_tar_gz);
    deb
}

fn fake_ikconfig_image(config: &str) -> Vec<u8> {
    let mut image = b"prefixIKCFG_ST".to_vec();
    image.extend_from_slice(&gzip_bytes(config));
    image.extend_from_slice(b"suffix");
    image
}

fn minimal_rpm_with_config(config: &str) -> Vec<u8> {
    let mut package_builder =
        PackageBuilder::new("kernel-core", "6.12.0", "MIT", "x86_64", "kernel");
    package_builder
        .release("1.fc99")
        .using_config(BuildConfig::v4().compression(CompressionType::Gzip))
        .with_file_contents(
            config.as_bytes(),
            FileOptions::new("/boot/config-6.12.0-1.fc99.x86_64"),
        )
        .expect("add config");
    let package = package_builder.build().expect("build rpm");
    let mut bytes = Vec::new();
    package.write(&mut bytes).expect("write rpm");
    bytes
}

fn gzip_bytes(input: &str) -> Vec<u8> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(input.as_bytes()).expect("write gzip");
    gz.finish().expect("finish gzip")
}

fn gzip_raw_bytes(input: &[u8]) -> Vec<u8> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(input).expect("write gzip");
    gz.finish().expect("finish gzip")
}

fn minimal_arch_package_with_config(config: &str) -> Vec<u8> {
    zstd::encode_all(
        tar_with_file(
            "usr/lib/modules/6.12.1-arch1-1/build/.config",
            config.as_bytes(),
        )
        .as_slice(),
        0,
    )
    .expect("write zstd")
}

fn tar_with_file(path: &str, contents: &[u8]) -> Vec<u8> {
    let mut tarball = Vec::new();
    {
        let mut builder = Builder::new(&mut tarball);
        let mut header = Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, path, contents)
            .expect("append file");
        builder.finish().expect("finish tar");
    }
    tarball
}

fn append_ar_member(ar: &mut Vec<u8>, name: &str, data: &[u8]) {
    let header = format!(
        "{:<16}{:<12}{:<6}{:<6}{:<8o}{:<10}`\n",
        name,
        0,
        0,
        0,
        0o100644,
        data.len()
    );
    assert_eq!(header.len(), 60);
    ar.extend_from_slice(header.as_bytes());
    ar.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        ar.push(b'\n');
    }
}
