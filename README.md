# kconfigwtf

kconfigwtf is a static Linux kernel config explorer. It builds an index of
distribution kernel packages and generates a static website where users can
search for a Kconfig entry such as `BPF`, see matching distribution kernel
packages, and open the raw kernel config that was indexed. The website also
accepts explicit `CONFIG_*` input such as `CONFIG_BPF`.

The foundation has two parts:

- A kernel config indexer layer with an extensible `KernelConfigIndexer` trait.
- A static site generator that renders a self-contained HTML/CSS/JavaScript
  search UI from package-level indexes under `data/`.

The first implemented distribution backends are Debian, Ubuntu, Kali, Proxmox,
Deepin, Kylin OS, OpenKylin, AOSC OS, Fedora, RHEL, CentOS Stream, AlmaLinux, Rocky Linux,
openAnolis, openEuler, openSUSE, Oracle Linux, Amazon Linux, Azure Linux,
Android AOSP GKI, Alpine Linux, NixOS, Guix,
and Arch-family pacman repositories for Arch Linux, Parabola, CachyOS, and
eweos, including the Arch Linux RISC-V repository as Arch Linux on `riscv64`.

## Install

```sh
cargo build --release
```

## Generate A Debian Index

Index Debian packages from a mirror:

```sh
cargo run -- index debian \
  --suite stable \
  --component main \
  --arch amd64 \
  --max-packages 25 \
  --data-dir data
```

The Debian backend reads the `Packages.gz` file for each architecture, selects
`linux-image-*` packages, downloads each `.deb`, extracts `/boot/config-*` or
falls back to embedded IKCONFIG from `/boot/vmlinuz-*`, and writes raw configs
plus package-level indexes. Use `--max-packages` during development to avoid
fetching a large number of packages.

Offline indexing is also supported for tests and mirror snapshots:

```sh
cargo run -- index debian \
  --packages-file ./mirror/dists/stable/main/binary-amd64/Packages \
  --deb-root ./mirror \
  --arch amd64 \
  --data-dir data
```

When `--packages-file` is used, `Filename` fields in the Packages file are
resolved relative to `--deb-root`.

## Generate Ubuntu, Kali, Proxmox, Deepin, Kylin, OpenKylin, And AOSC OS Indexes

Ubuntu, Kali, Proxmox, Deepin, Kylin OS, OpenKylin, and AOSC OS use the same APT package index and `.deb` extraction
machinery as Debian, with distribution-specific defaults:

```sh
cargo run -- index ubuntu \
  --suite noble-updates \
  --component main \
  --arch amd64 \
  --max-packages 5 \
  --data-dir data

cargo run -- index kali \
  --suite kali-rolling \
  --component main \
  --arch amd64 \
  --max-packages 5 \
  --data-dir data

cargo run -- index proxmox \
  --suite bookworm \
  --component pve-no-subscription \
  --arch amd64 \
  --max-packages 5 \
  --data-dir data

cargo run -- index deepin \
  --suite beige \
  --component main \
  --arch amd64 \
  --max-packages 5 \
  --data-dir data

cargo run -- index kylin \
  --suite 10.1 \
  --component main \
  --arch amd64 \
  --max-packages 5 \
  --data-dir data

cargo run -- index openkylin \
  --suite nile.bedrock \
  --component main \
  --arch amd64 \
  --max-packages 5 \
  --data-dir data

cargo run -- index aosc \
  --suite stable \
  --component main \
  --arch amd64 \
  --max-packages 5 \
  --data-dir data
```

Default mirrors are:

- Ubuntu: `http://archive.ubuntu.com/ubuntu`
- Kali: `http://http.kali.org/kali`
- Proxmox: `http://download.proxmox.com/debian/pve`
- Deepin: `https://community-packages.deepin.com/beige`
- Kylin: `https://archive.kylinos.cn/kylin/KYLIN-ALL`
- OpenKylin: `https://archive.openkylin.top/openkylin`
- AOSC OS: `https://repo.aosc.io/debs`

The Ubuntu backend selects `linux-modules-*` packages, and the Kali backend
selects `linux-base-*` packages, because those packages carry `/boot/config-*`
in current repositories. Package names are normalized back to `linux-image-*`
in the generated data and UI. The Proxmox backend selects unsigned
`proxmox-kernel-*-pve` packages and skips signed, signed-template, and meta
packages that do not directly provide a config. The AOSC OS backend selects
`linux-kernel-*` packages and extracts embedded IKCONFIG from `/boot/vmlinuz-*`
when a standalone `/boot/config-*` file is not present.

Offline indexing works the same as Debian:

```sh
cargo run -- index ubuntu \
  --packages-file ./mirror/dists/noble-updates/main/binary-amd64/Packages \
  --deb-root ./mirror \
  --arch amd64 \
  --data-dir data
```

## Generate An RPM-Family Index

Index Fedora `kernel-core` packages from a mirror:

```sh
cargo run -- index fedora \
  --release rawhide \
  --arch x86_64 \
  --max-packages 5 \
  --data-dir data
```

The same RPM backend also supports RHEL, CentOS Stream, AlmaLinux, Rocky Linux,
openAnolis, openEuler, openSUSE, Oracle Linux, Amazon Linux, and Azure Linux:

```sh
cargo run -- index centos --release 10-stream --max-packages 5 --data-dir data
cargo run -- index centos --release 6 --max-packages 5 --data-dir data
cargo run -- index almalinux --release 10 --max-packages 5 --data-dir data
cargo run -- index rocky --release 10 --max-packages 5 --data-dir data
cargo run -- index openanolis --release 23.1 --max-packages 5 --data-dir data
cargo run -- index openeuler --release openEuler-24.03-LTS --max-packages 5 --data-dir data
cargo run -- index opensuse --release tumbleweed --max-packages 5 --data-dir data
cargo run -- index oraclelinux --release 9 --max-packages 5 --data-dir data
cargo run -- index amazonlinux --release al2023 --max-packages 5 --data-dir data
cargo run -- index azurelinux --release 3.0 --max-packages 5 --data-dir data
```

CentOS Stream releases use `mirror.stream.centos.org`. Archived CentOS
releases use `vault.centos.org`; shorthand releases `6`, `7`, and `8` resolve
to the final archived point releases `6.10`, `7.9.2009`, and `8.5.2111`.

RHEL uses the Red Hat CDN path by default and requires an entitled environment
or an accessible mirror:

```sh
cargo run -- index rhel \
  --mirror https://cdn.redhat.com/content/dist \
  --release 10 \
  --max-packages 5 \
  --data-dir data
```

The RPM backend reads `repodata/repomd.xml`, follows the primary metadata
location, selects matching RPMs, extracts `/boot/config-*` or
`/lib/modules/*/config` from each package, and writes raw configs plus
package-level indexes. Use `--max-packages` during development to avoid
downloading many large kernel RPMs. The default package is `kernel-core` for
Fedora and modern Enterprise Linux distributions, `kernel` for CentOS 6/7,
Amazon Linux, and `kernel` for openAnolis and openEuler. openSUSE defaults to
`kernel-default`.
openSUSE also indexes additional kernel flavors by default, including
`kernel-vanilla`, `kernel-longterm`, and `kernel-kvmsmall`.

Offline indexing is also supported for tests and mirror snapshots:

```sh
cargo run -- index fedora \
  --repomd-file ./mirror/repodata/repomd.xml \
  --rpm-root ./mirror \
  --arch x86_64 \
  --data-dir data
```

When `--repomd-file` is used, primary metadata and RPM `href` fields are
resolved relative to `--rpm-root`.

## Generate An Android AOSP GKI Index

Index Android GKI release builds from AOSP release metadata. By default, the
CLI discovers release-build branches from the Source Android GKI overview page:

```sh
cargo run -- index android \
  --max-builds 5 \
  --data-dir data
```

Use `--branch` one or more times to index a selected subset:

```sh
cargo run -- index android \
  --branch android16-6.12 \
  --branch android15-6.6 \
  --max-builds 5 \
  --data-dir data
```

The Android backend reads the Source Android GKI release-build JSON pages, then
checks Android CI `BUILD_INFO` for each selected build. Newer builds publish
`kernel_aarch64_dot_config` directly; older builds only provide `boot.img`, so
the backend extracts IKCONFIG with the bundled `scripts/extract-ikconfig`
helper. The distribution is stored as `android`, and the package name is the
branch name from the release metadata, for example `android16-6.12`.

Offline indexing is also supported for tests and snapshots:

```sh
cargo run -- index android \
  --release-builds-file ./gki-android16-6_12-release-builds.html \
  --artifact-root ./artifacts \
  --max-builds 1 \
  --data-dir data
```

When `--release-builds-file` is used, configs are resolved as
`<artifact-root>/<kernel_bid>/<target>/<config-artifact>`.

## Generate An Arch-Family Index

Index Arch Linux kernel packages from a pacman mirror:

```sh
cargo run -- index arch \
  --distribution archlinux \
  --repository core \
  --arch x86_64 \
  --max-packages 5 \
  --data-dir data
```

The Arch-family backend reads a pacman sync database such as `core.db`, selects
kernel development packages matching `--package-prefix` (default `linux`),
downloads each `.pkg.tar.*` package, extracts kernel config files such as
`usr/lib/modules/*/build/.config` or `usr/src/linux/.config`, and writes raw
configs plus package-level indexes. Arch Linux stores the build config in
packages such as `linux-headers`, while eweOS uses packages such as
`linux-devel`; the backend strips the `-headers` or `-devel` suffix when
writing the data tree and UI package name.

Parabola and CachyOS use the same backend with distro-specific default mirror
and repository values. Arch Linux RISC-V is treated as Arch Linux on the
`riscv64` architecture, so `--arch riscv64` selects the Arch RISC-V repository
defaults and stores data under `archlinux`:

```sh
cargo run -- index arch --distribution parabola --arch x86_64 --data-dir data
cargo run -- index arch --distribution cachyos --arch x86_64_v4 --repository cachyos-znver4 --data-dir data
cargo run -- index arch --arch riscv64 --data-dir data
cargo run -- index eweos --repository main --arch x86_64 --data-dir data
```

Offline indexing is also supported for tests and mirror snapshots:

```sh
cargo run -- index arch \
  --distribution archlinux \
  --db-file ./mirror/core/os/x86_64/core.db \
  --package-root ./mirror/core/os/x86_64 \
  --arch x86_64 \
  --data-dir data
```

## Generate An Alpine Index

Index Alpine Linux kernel packages from an apk repository:

```sh
cargo run -- index alpine \
  --release latest-stable \
  --repository main \
  --repository community \
  --arch x86_64 \
  --max-packages 5 \
  --data-dir data
```

The Alpine backend reads `APKINDEX.tar.gz` from each selected repository. The
default repositories are `main` and `community`, so edge/community kernels such
as `linux-stable` are included. It selects kernel packages matching
`--package-prefix` (default `linux-`) while skipping development, doc, tools,
firmware, and similar companion packages, downloads each `.apk`, extracts
`/boot/config-*` or `usr/src/*/.config`, and writes raw configs plus
package-level indexes.

When `--db-file` is used, package filenames from the sync database are resolved
relative to `--package-root`.

## Generate NixOS And Guix Indexes

NixOS and Guix are indexed through their native package manager CLIs. The
backend resolves each requested package to a store output, scans the output for
kernel configs, and falls back to embedded IKCONFIG in kernel images such as
Nix's `bzImage`.

```sh
cargo run -- index nixos \
  --arch x86_64 \
  --max-packages 1 \
  --data-dir data

cargo run -- index guix \
  --package linux-libre \
  --arch x86_64 \
  --max-packages 1 \
  --data-dir data
```

NixOS discovers derivation-valued attributes under
`nixpkgs#linuxKernel.kernels` by default and also includes
`linuxPackages_latest.kernel`, `linux_zen`, `linux`, `linux_latest`, and
`linux_xanmod`. Use `--flake` to select another flake, or pass `--package` one
or more times to index an explicit subset. Guix defaults to `linux-libre`.
Both commands accept `--system` when the package manager system string should
differ from the selected output architecture.

## Generate The Static Site

```sh
cargo run -- site \
  --data-dir data \
  --output-dir public \
  --title kconfigwtf
```

The generated site consists of:

- `index.html`
- `app.js`
- `styles.css`
- `indexes.json`
- `data/`, copied from the indexed data directory
- `CONFIG_/<ENTRY>/index.html`, one generated result page per Kconfig entry

Because the site uses `fetch`, serve it with any static file
server instead of opening `index.html` directly from disk:

```sh
python3 -m http.server 8000 --directory public
```

Then open `http://localhost:8000`.

The search box autocompletes Kconfig names from a pre-generated list in
`indexes.json`. Suggestions are shown without the `CONFIG_` prefix unless the
user has already typed it. Submitting the search navigates to the pre-generated
page, for example `http://localhost:8000/CONFIG_/BPF/`.

## Architecture

The crate is split into focused modules:

- `index`: JSON schema, config parser, normalization, and index aggregation.
- `indexer`: shared distribution indexer trait and package payload type.
- `android`: Android AOSP GKI release metadata parser and Android CI
  `kernel_aarch64_dot_config` retriever.
- `alpine`: Alpine `APKINDEX.tar.gz` parser, `.apk` extraction, and indexer
  implementation.
- `arch`: Arch-family pacman sync database parser, `.pkg.tar.*` extraction, and
  indexer implementation for Arch Linux, Parabola, CachyOS, and eweOS.
- `debian`: APT `Packages` parser, package selection, `.deb` extraction, and
  indexer implementation used by Debian, Ubuntu, Kali, Proxmox, Deepin, Kylin, OpenKylin, and AOSC OS.
- `fedora`: Fedora and RPM-family `repomd.xml` / primary metadata parser, RPM
  extraction, and indexer implementation for Fedora, RHEL, CentOS Stream,
  AlmaLinux, Rocky Linux, openAnolis, openEuler, openSUSE, Oracle Linux,
  Amazon Linux, and Azure Linux.
- `store`: NixOS and Guix package-manager backed indexing from store outputs.
- `site`: static site rendering using MiniJinja templates.

Distribution backends implement:

```rust
#[async_trait::async_trait]
pub trait KernelConfigIndexer: Send + Sync {
    async fn index(&self) -> anyhow::Result<Vec<KernelConfigPackage>>;
}
```

Backends return raw kernel config text with typed package metadata. The shared
data writer stores each raw config and writes one package-level index per
distribution/package pair.

## Data Layout

Indexed data is intended to live in this repository:

```text
data/<DISTRO>/<PACKAGE>/<VERSION>/<ARCH>/config
data/<DISTRO>/<PACKAGE>/index.json
```

For example:

```text
data/debian/linux-image-<VERSION>-<ARCH>/6.1.4-1/amd64/config
data/debian/linux-image-<VERSION>-<ARCH>/index.json
data/fedora/kernel-core/0:6.12.0-1.fc99/amd64/config
data/fedora/kernel-core/index.json
data/archlinux/linux/6.12.1.arch1-1/amd64/config
data/archlinux/linux/index.json
data/android/android16-6.12/android16-6.12-2026-03_r32/arm64/config
data/android/android16-6.12/index.json
data/ubuntu/linux-image-<VERSION>-generic/6.14.0-29.29~24.04.1/amd64/config
data/proxmox/proxmox-kernel-<VERSION>-pve/6.11.0-1/amd64/config
```

Each package index stores package metadata once and refers to kernels by a
compact kernel key, so `distribution` and `package_name` are not repeated for
every Kconfig entry:

```json
{
  "schema_version": 4,
  "generated_at": "2026-05-20T00:00:00Z",
  "distribution": "debian",
  "package_name": "linux-image-<VERSION>-<ARCH>",
  "kernels": {
    "6.1.4-1/amd64": {
      "version": "6.1.4-1",
      "architecture": "amd64",
      "config_path": "6.1.4-1/amd64/config",
      "source": "https://deb.debian.org/debian/pool/main/l/linux/linux-image.deb#boot/config-6.1.0-1-amd64"
    }
  },
  "entries": {
    "CONFIG_BPF": [
      {
        "kernel": "6.1.4-1/amd64",
        "value": "built_in"
      }
    ]
  }
}
```

`value` is one of:

- `"built_in"` for `CONFIG_FOO=y`
- `"module"` for `CONFIG_FOO=m`
- `"-"` for `# CONFIG_FOO is not set`
- `{ "other": "..." }` for string, numeric, or other assigned values

`distribution` and `architecture` are represented as Rust enums and serialized
as stable lowercase strings in JSON. Known values include `debian` for
distribution plus `android`, `ubuntu`, `kali`, `proxmox`, `deepin`, `kylin`,
`aoscos`, `archlinux`, `parabola`, `cachyos`, `eweos`, `alpine`, `nixos`,
`guix`, `fedora`, `rhel`, `centos`, `almalinux`, `rocky`, `openanolis`,
`openeuler`, `openkylin`, `opensuse`, `oraclelinux`, `amazonlinux`, and
`azurelinux`.
Architectures include `amd64`, `arm64`, `armhf`, `i386`,
`ppc64el`, `riscv64`, and `s390x`. Unknown future values are preserved through
an `Other(String)` enum variant.

The static site generator scans `data/**/index.json`, validates those package
indexes, copies the data tree into the site output, writes `indexes.json`, and
generates `CONFIG_/<ENTRY>/index.html` result pages. The manifest contains both
the package index URLs and a sorted list of available Kconfig names for
autocomplete, avoiding a browser-side full index scan before search.

## Test

```sh
cargo test
```

The test suite includes unit tests for config parsing, Debian Packages parsing,
Fedora repository metadata, pacman sync databases, package extraction, and site
generation, plus integration tests for the CLI.
