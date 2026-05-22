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

## Arch-Family Backend

The Arch-family backend supports Arch Linux, Parabola, and CachyOS through the
same pacman repository implementation. It supports two retrieval modes:

- Mirror mode, using a pacman sync database such as
  `<repo>/os/<arch>/<repo>.db` for Arch Linux and Parabola.
- Local mode, using `--db-file` and resolving package filenames under
  `--package-root`.

CachyOS uses the same pacman database format but defaults to the
`<repo>/<arch>/<repo>.db` mirror layout.

The backend parses package `desc` files from the sync database, selects
`*-headers` package names matching `--package-prefix`, and extracts config
files from `.pkg.tar.*` packages. Arch Linux stores the build config in header
packages such as `linux-headers`; the backend strips the `-headers` suffix from
the indexed package name so the data tree and UI show the kernel package name
(`linux`, `linux-lts`, `linux-cachyos`, and similar names). Supported archive
compression formats are:

- `.pkg.tar`
- `.pkg.tar.gz`
- `.pkg.tar.xz`
- `.pkg.tar.zst` / `.pkg.tar.zstd`

The backend currently looks for kernel configs under paths such as
`usr/lib/modules/*/build/.config`, `lib/modules/*/build/.config`, and
`boot/config-*`.

## Debian Backend

The Debian backend supports two retrieval modes:

- Mirror mode, using `dists/<suite>/<component>/binary-<arch>/Packages.gz`.
- Local mode, using `--packages-file` and resolving package `Filename` values
  under `--deb-root`.

The backend currently extracts config files from these Debian data archive
formats:

- `data.tar`
- `data.tar.gz`
- `data.tar.xz`
- `data.tar.zst`
- `data.tar.zstd`

Future Debian improvements can add source package metadata, package version
ordering, snapshot pinning, and stricter kernel image package filtering.

## Fedora Backend

The Fedora backend supports two retrieval modes:

- Mirror mode, using `repodata/repomd.xml` and the referenced primary metadata.
- Local mode, using `--repomd-file` and resolving primary metadata and RPM
  `href` values under `--rpm-root`.

The backend selects matching RPM package names, currently defaulting to
`kernel-core`, and extracts `/boot/config-*` or `lib/modules/*/config` from RPM
payloads.

## Adding Another Distribution

Add a module that implements `KernelConfigIndexer`, then wire it into the CLI as
a new `index <distribution>` subcommand. Keep retrieval and package parsing in
the backend, but reuse `write_packages_to_data_dir` for config persistence and
JSON generation.
