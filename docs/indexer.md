# Indexer Design

The indexer layer keeps distribution-specific retrieval separate from the shared
index format and static site generator.

## Data Flow

1. A distribution backend implements `KernelConfigIndexer`.
2. The backend retrieves kernel package metadata and raw kernel config files.
3. The backend returns `KernelConfigPackage` values.
4. `ConfigIndex::from_packages` parses enabled `CONFIG_*` entries.
5. The CLI writes `index.json`.
6. The site generator copies that JSON into a static site.

## Distribution Backend Contract

Backends should populate:

- `distribution`: stable lowercase distribution id, for example `debian`.
- `package_name`: the binary package that shipped the config.
- `package_version`: the binary package version.
- `architecture`: the CPU architecture represented by the config.
- `source`: optional URL or path to the package and config file.
- `config_text`: raw Linux kernel config text.

Backends should not emit one record per config entry. They should emit one
record per discovered kernel config file and let the shared index builder parse
and aggregate entries.

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

## Adding Another Distribution

Add a module that implements `KernelConfigIndexer`, then wire it into the CLI as
a new `index <distribution>` subcommand. Keep retrieval and package parsing in
the backend, but reuse `ConfigIndex` for config parsing and JSON generation.
