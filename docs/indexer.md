# Indexer Design

The indexer layer keeps distribution-specific retrieval separate from the shared
index format and static site generator.

## Data Flow

1. A distribution backend implements `KernelConfigIndexer`.
2. The backend retrieves kernel package metadata and raw kernel config files.
3. The backend returns `KernelConfigPackage` values.
4. `write_packages_to_data_dir` writes raw configs to
   `data/<DISTRO>/<PACKAGE>/<VERSION>/<ARCH>/config`.
5. The same data writer builds `data/<DISTRO>/<PACKAGE>/index.json`.
6. The site generator scans `data/**/index.json`, copies the data tree, writes a
   static-site manifest containing package index URLs plus the complete Kconfig
   name list for autocomplete, and generates one `CONFIG_/<ENTRY>/index.html`
   result page per Kconfig entry.

## Distribution Backend Contract

Backends should populate:

- `distribution`: typed `Distribution` enum value, for example
  `Distribution::Debian` or `Distribution::ArchLinux`.
- `package_name`: the binary package that shipped the config.
- `package_version`: the binary package version.
- `architecture`: typed `Architecture` enum value, for example
  `Architecture::Amd64`.
- `source`: optional URL or path to the package and config file.
- `config_text`: raw Linux kernel config text.

Backends should not emit one record per config entry. They should emit one
record per discovered kernel config file and let the shared data writer persist
raw configs and build package-level indexes.

Package-level indexes store `distribution` and `package_name` once. Each
`entries` occurrence points at a kernel key such as `6.1.4-1/amd64`, and the
`kernels` map stores the version, architecture, config path, and source for
that kernel.

`Distribution::Other(String)` and `Architecture::Other(String)` keep the model
extensible when adding distributions or architectures that do not have a named
variant yet.

Distribution backends may normalize package names before returning
`KernelConfigPackage` values when the native package name embeds volatile kernel
details. The Debian backend replaces the kernel version and architecture in
`linux-image-*` package names with `<VERSION>` and `<ARCH>` so related kernels
share one package-level index.

## Alpine Backend

The Alpine backend supports apk repositories. It supports two retrieval modes:

- Mirror mode, using
  `<mirror>/<release>/<repository>/<arch>/APKINDEX.tar.gz`. The CLI accepts
  `--repository` more than once and defaults to `main` plus `community`, which
  includes edge/community packages such as `linux-stable`.
- Local mode, using `--apkindex-file` and resolving package filenames under
  `--apk-root`.

The backend parses the `APKINDEX` file inside `APKINDEX.tar.gz`, selects kernel
packages matching `--package-prefix` while skipping development, documentation,
tools, firmware, and similar companion packages, and extracts config files from
`.apk` packages. The backend currently looks for kernel configs under paths
such as `boot/config-*` and `usr/src/*/.config`.

## Arch-Family Backend

The Arch-family backend supports Arch Linux, Arch Linux RISC-V, Parabola,
CachyOS, and eweOS through the same pacman repository implementation. It
supports two retrieval modes:

- Mirror mode, using a pacman sync database such as
  `<repo>/os/<arch>/<repo>.db` for Arch Linux and Parabola.
- Local mode, using `--db-file` and resolving package filenames under
  `--package-root`.

CachyOS uses the same pacman database format but defaults to the
`<repo>/<arch>/<repo>.db` mirror layout. Arch Linux on `riscv64` uses the flat
Arch RISC-V `<repo>/<repo>.db` mirror layout and is represented as the
`archlinux` distribution in generated data.

The backend parses package `desc` files from the sync database, selects
`*-headers` or `*-devel` package names matching `--package-prefix`, and
extracts config files from `.pkg.tar.*` packages. Arch Linux stores the build
config in header packages such as `linux-headers`; eweOS stores it in
development packages such as `linux-devel`. The backend strips the `-headers`
or `-devel` suffix from the indexed package name so the data tree and UI show
the kernel package name (`linux`, `linux-lts`, `linux-cachyos`, and similar
names). Supported archive compression formats are:

- `.pkg.tar`
- `.pkg.tar.gz`
- `.pkg.tar.xz`
- `.pkg.tar.zst` / `.pkg.tar.zstd`

The backend currently looks for kernel configs under paths such as
`usr/lib/modules/*/build/.config`, `lib/modules/*/build/.config`, and
`usr/src/*/.config`, and `boot/config-*`.

## Android AOSP GKI Backend

The Android backend indexes AOSP GKI release builds. It supports three
retrieval modes:

- Discovery mode, using the Source Android GKI overview page to discover
  release-build branches, then fetching each branch release-build JSON page.
- Selected branch mode, using `--branch` one or more times to fetch specific
  release-build JSON pages.
- Local mode, using `--release-builds-file` and resolving configs under
  `--artifact-root`. Offline discovery tests can also use `--discovery-file`
  and `--release-builds-root`.

The release-build metadata lists Android CI build IDs as `kernel_bid`. For each
selected build, the backend reads Android CI `BUILD_INFO` and tries config
sources in order: `kernel_aarch64_dot_config`, then `boot.img`, `boot-gz.img`,
`vmlinux`, `Image`, and `boot-lz4.img`. Sources other than the dot-config file
are decoded with the bundled `scripts/extract-ikconfig` helper. Some older
branches (for example `android12-5.10`) publish only `vmlinux`/`Image` and not
`boot.img`.

```text
https://ci.android.com/builds/submitted/<kernel_bid>/kernel_aarch64/latest/raw/kernel_aarch64_dot_config
https://ci.android.com/builds/submitted/<kernel_bid>/kernel_aarch64/latest/raw/boot.img
```

Missing `kernel_aarch64_dot_config` URLs redirect to an HTML artifact browser
page; the backend ignores that response and uses the `boot.img` fallback
instead.
The backend stores `Distribution::Android`, uses the branch name from release
metadata as the package name, and uses the release tag as the package version.

## Debian Backend

The Debian backend is an APT repository backend used by Debian, Ubuntu, Kali,
Proxmox, Deepin, Kylin OS, and AOSC OS. It supports two retrieval modes:

- Mirror mode, using `dists/<suite>/<component>/binary-<arch>/Packages.gz` or `Packages.xz`.
- Local mode, using `--packages-file` and resolving package `Filename` values
  under `--deb-root`.

The backend currently extracts config files from these Debian data archive
formats:

- `data.tar`
- `data.tar.gz`
- `data.tar.xz`
- `data.tar.zst`
- `data.tar.zstd`

It first looks for standalone `boot/config-*` files. If none are present, it
tries embedded IKCONFIG from kernel images such as `boot/vmlinuz-*`, which is
needed for AOSC OS `linux-kernel-*` packages.

Future Debian improvements can add source package metadata, package version
ordering, snapshot pinning, and stricter kernel image package filtering.

## Ubuntu, Kali, And Proxmox Backends

Ubuntu, Kali, and Proxmox reuse the APT backend with different distribution
values and defaults:

- Ubuntu defaults to `http://archive.ubuntu.com/ubuntu`,
  `noble-updates`, `main`, and `linux-modules-`.
- Kali defaults to `http://http.kali.org/kali`, `kali-rolling`, `main`, and
  `linux-base-`.
- Proxmox defaults to `http://download.proxmox.com/debian/pve`, `bookworm`,
  `pve-no-subscription`, and `proxmox-kernel-`.

The Proxmox CLI requires package names to contain `-pve` and excludes signed
and signed-template variants by default, so package-level indexes are built
from unsigned kernel image packages rather than meta packages.

APT-style package names that embed the kernel version or architecture are
normalized before indexing. Ubuntu configs are extracted from `linux-modules-*`
packages, Kali configs are extracted from `linux-base-*` packages, and both are
displayed as `linux-image-*` package names. For example,
`linux-modules-6.14.0-29-generic` becomes `linux-image-<VERSION>-generic`, and
`linux-base-6.19.14+kali-amd64` becomes `linux-image-<VERSION>-<ARCH>`.
`proxmox-kernel-6.11.0-1-pve` becomes `proxmox-kernel-<VERSION>-pve`.

## RPM-Family Backend

The RPM-family backend is implemented in the Fedora module and supports Fedora,
RHEL, CentOS Stream, AlmaLinux, Rocky Linux, openAnolis, openEuler, and
openSUSE. It supports two retrieval modes:

- Mirror mode, using `repodata/repomd.xml` and the referenced primary metadata.
- Local mode, using `--repomd-file` and resolving primary metadata and RPM
  `href` values under `--rpm-root`.

The backend selects matching RPM package names, currently defaulting to
`kernel-core` for Fedora and modern Enterprise Linux distributions, `kernel`
for CentOS 6/7, and `kernel` for openAnolis and openEuler. openSUSE defaults
to `kernel-default`, `kernel-vanilla`, `kernel-longterm`, and
`kernel-kvmsmall`. It extracts `/boot/config-*` or `lib/modules/*/config` from
RPM payloads.

Default mirror layouts are:

- Fedora: `<mirror>/releases/<release>/Everything/<arch>/os`, or
  `<mirror>/development/rawhide/Everything/<arch>/os` for rawhide.
- RHEL: `<mirror>/rhel<major>/<release>/<arch>/<repo>/os`, where the default
  mirror is the Red Hat CDN and requires entitlement or an accessible mirror.
- CentOS 6/7: `<mirror>/<release>/<repo>/<arch>`, using `vault.centos.org` by
  default. Shorthand releases `6` and `7` resolve to `6.10` and `7.9.2009`.
- CentOS 8: `<mirror>/<release>/<repo>/<arch>/os`, using `vault.centos.org` by
  default. Shorthand release `8` resolves to `8.5.2111`.
- CentOS Stream: `<mirror>/<release>/<repo>/<arch>/os`, using
  `mirror.stream.centos.org` by default.
- AlmaLinux and Rocky Linux: `<mirror>/<release>/<repo>/<arch>/os`.
- openAnolis: `<mirror>/<release>/<repo>/<arch>/os`, defaulting to release
  `23.1`, repository `os`, and package `kernel`. Release `8` defaults to
  repository `BaseOS`.
- openEuler: `<mirror>/<release>/<repo>/<arch>`.
- openSUSE Tumbleweed: `<mirror>/tumbleweed/repo/<repo>`.
- openSUSE Leap: `<mirror>/distribution/leap/<release>/repo/<repo>`.

## Store Package Backends

The NixOS and Guix backends use local package-manager commands rather than
repository metadata:

- NixOS discovers derivation-valued attributes under `linuxKernel.kernels`,
  adds common top-level aliases such as `linux`, `linux_latest`, `linux_zen`,
  `linux_xanmod`, and `linuxPackages_latest.kernel`, then runs
  `nix build --no-link --print-out-paths` for each selected package attribute
  and tries `nix eval --raw <installable>.version` for the package version.
- Guix runs `guix build` for each requested package and derives the package
  version from the resulting store path.

Both backends scan the resolved store output for config files such as
`lib/modules/*/build/.config`, `lib/modules/*/config`, and `config-*`. If no
plain config file is present, they try embedded IKCONFIG from kernel images
such as `bzImage`, `Image`, and `vmlinuz-*`.

## Adding Another Distribution

Add a module that implements `KernelConfigIndexer`, then wire it into the CLI as
a new `index <distribution>` subcommand. Keep retrieval and package parsing in
the backend, but reuse `write_packages_to_data_dir` for config persistence and
JSON generation.
