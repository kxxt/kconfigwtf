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
Fedora, Android AOSP GKI, and Arch-family pacman repositories for Arch Linux,
Parabola, and CachyOS.

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
`linux-image-*` packages, downloads each `.deb`, extracts `/boot/config-*`, and
writes raw configs plus package-level indexes. Use `--max-packages` during
development to avoid fetching a large number of packages.

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

## Generate Ubuntu, Kali, And Proxmox Indexes

Ubuntu, Kali, and Proxmox use the same APT package index and `.deb` extraction
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
```

Default mirrors are:

- Ubuntu: `http://archive.ubuntu.com/ubuntu`
- Kali: `http://http.kali.org/kali`
- Proxmox: `http://download.proxmox.com/debian/pve`

The Ubuntu backend selects `linux-modules-*` packages, and the Kali backend
selects `linux-base-*` packages, because those packages carry `/boot/config-*`
in current repositories. Package names are normalized back to `linux-image-*`
in the generated data and UI. The Proxmox backend selects unsigned
`proxmox-kernel-*-pve` packages and skips signed, signed-template, and meta
packages that do not directly provide a config.

Offline indexing works the same as Debian:

```sh
cargo run -- index ubuntu \
  --packages-file ./mirror/dists/noble-updates/main/binary-amd64/Packages \
  --deb-root ./mirror \
  --arch amd64 \
  --data-dir data
```

## Generate A Fedora Index

Index Fedora `kernel-core` packages from a mirror:

```sh
cargo run -- index fedora \
  --release rawhide \
  --arch x86_64 \
  --max-packages 5 \
  --data-dir data
```

The Fedora backend reads `repodata/repomd.xml`, follows the primary metadata
location, selects matching RPMs, extracts `/boot/config-*` from each package,
and writes raw configs plus package-level indexes. Use `--max-packages` during
development to avoid downloading many large kernel RPMs.

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
kernel header packages matching `--package-prefix` (default `linux`), downloads
each `.pkg.tar.*` package, extracts kernel config files such as
`usr/lib/modules/*/build/.config`, and writes raw configs plus package-level
indexes. Arch Linux stores the build config in packages such as
`linux-headers`, so the backend strips the `-headers` suffix when writing the
data tree and UI package name.

Parabola and CachyOS use the same backend with distro-specific default mirror
and repository values:

```sh
cargo run -- index arch --distribution parabola --arch x86_64 --data-dir data
cargo run -- index arch --distribution cachyos --arch x86_64 --data-dir data
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

When `--db-file` is used, package filenames from the sync database are resolved
relative to `--package-root`.

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
- `arch`: Arch-family pacman sync database parser, `.pkg.tar.*` extraction, and
  indexer implementation for Arch Linux, Parabola, and CachyOS.
- `debian`: APT `Packages` parser, package selection, `.deb` extraction, and
  indexer implementation used by Debian, Ubuntu, Kali, and Proxmox.
- `fedora`: Fedora `repomd.xml` / primary metadata parser, RPM extraction, and
  indexer implementation.
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
distribution plus `android`, `ubuntu`, `kali`, `proxmox`, `archlinux`,
`parabola`, `cachyos`, and `fedora`. Architectures include `amd64`, `arm64`,
`armhf`, `i386`, `ppc64el`, `riscv64`, and `s390x`. Unknown future values are
preserved through an `Other(String)` enum variant.

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
