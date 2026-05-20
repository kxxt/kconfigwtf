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

The first implemented distribution backend is Debian.

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
- `debian`: Debian `Packages` parser, package selection, `.deb` extraction, and
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
distribution and Debian architectures such as `amd64`, `arm64`, `armhf`, `i386`,
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
`.deb` extraction, and site generation, plus integration tests for the CLI.
