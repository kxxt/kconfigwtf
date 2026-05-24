# kconfigwtf

kconfigwtf is a static Linux kernel config explorer. It collects kernel config
files from many distributions, stores them in a normalized `data/` tree, and
builds a static website for searching Kconfig entries such as `BPF` or
`CONFIG_BPF`.

The repository includes:

- a checked-in `data/` directory with generated package indexes and raw configs
- a static site generator that turns that data into a browser UI
- distribution-specific indexers for refreshing or extending the dataset

Supported sources currently include Debian-family distributions, Fedora and
other RPM-based distributions, Arch-family repositories, Alpine, Android GKI,
OpenWrt, Slackware, NixOS, Guix, Void Linux, ChromeOS recovery images, and
more.

## Use It

If this repository has GitHub Pages enabled, the easiest way to use kconfigwtf
is the published static site for this repo.

You can also build and browse the site locally from the checked-in `data/`
directory:

```sh
cargo run -- site \
  --data-dir data \
  --output-dir public \
  --title kconfigwtf

python3 -m http.server 8000 --directory public
```

Then open `http://localhost:8000`.

## Refresh The Data

If you want to regenerate part of the dataset, run one of the indexers and then
rebuild the site. Example:

```sh
cargo run -- index void \
  --package linux6.6 \
  --arch amd64 \
  --data-dir data

cargo run -- site \
  --data-dir data \
  --output-dir public \
  --title kconfigwtf
```

Each backend has its own flags and data source expectations.

## Documentation

- Developer guide: [docs/developer-guide.md](./docs/developer-guide.md)
- Indexer design: [docs/indexer.md](./docs/indexer.md)

The developer guide contains the full backend-by-backend indexing instructions,
site generation notes, CI/deployment details, and project architecture.
