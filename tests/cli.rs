use std::fs;
use std::io::Write;

use assert_cmd::Command;
use flate2::Compression;
use flate2::write::GzEncoder;
use kconfigwtf::index::{
    Architecture, ConfigValue, Distribution, PackageIndex, write_packages_to_data_dir,
};
use kconfigwtf::indexer::KernelConfigPackage;
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

fn minimal_deb_with_config(config: &str) -> Vec<u8> {
    let mut tarball = Vec::new();
    {
        let mut builder = Builder::new(&mut tarball);
        let mut header = Header::new_gnu();
        header.set_size(config.len() as u64);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                "./boot/config-6.1.0-1-amd64",
                config.as_bytes(),
            )
            .expect("append config");
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
