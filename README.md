# kconfigwtf

kconfigwtf is a static Linux kernel config explorer. It builds an index of
distribution kernel packages and generates a static website where users can
search for a `CONFIG_*` entry and see which distribution enabled it, in which
kernel package, at what version, and for which CPU architecture.

The foundation has two parts:

- A kernel config indexer layer with an extensible `KernelConfigIndexer` trait.
- A static site generator that renders a self-contained HTML/CSS/JavaScript
  search UI from the index JSON.

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
  --output dist/index.json
```

The Debian backend reads the `Packages.gz` file for each architecture, selects
`linux-image-*` packages, downloads each `.deb`, extracts `/boot/config-*`, and
indexes enabled config entries. Use `--max-packages` during development to avoid
fetching a large number of packages.

Offline indexing is also supported for tests and mirror snapshots:

```sh
cargo run -- index debian \
  --packages-file ./mirror/dists/stable/main/binary-amd64/Packages \
  --deb-root ./mirror \
  --arch amd64 \
  --output dist/index.json
```

When `--packages-file` is used, `Filename` fields in the Packages file are
resolved relative to `--deb-root`.

## Generate The Static Site

```sh
cargo run -- site \
  --index dist/index.json \
  --output-dir public \
  --title kconfigwtf
```

The generated site consists of:

- `index.html`
- `app.js`
- `styles.css`
- `index.json`

Because the site uses `fetch("index.json")`, serve it with any static file
server instead of opening `index.html` directly from disk:

```sh
python3 -m http.server 8000 --directory public
```

Then open `http://localhost:8000`.

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
`ConfigIndex` builder parses assigned entries and missing-value entries and
writes the common JSON format.

## Index Format

The generated JSON is intentionally static-site friendly:

```json
{
  "schema_version": 3,
  "generated_at": "2026-05-20T00:00:00Z",
  "entries": {
    "CONFIG_BPF": [
      {
        "distribution": "debian",
        "package_name": "linux-image-6.1.0-1-amd64",
        "package_version": "6.1.4-1",
        "architecture": "amd64",
        "value": "built_in",
        "source": "https://deb.debian.org/debian/pool/main/l/linux/linux-image.deb#boot/config-6.1.0-1-amd64"
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

## Test

```sh
cargo test
```

The test suite includes unit tests for config parsing, Debian Packages parsing,
`.deb` extraction, and site generation, plus integration tests for the CLI.
