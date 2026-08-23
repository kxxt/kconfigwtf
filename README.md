# kconfigwtf

kconfigwtf is a Linux kernel config explorer. It collects kernel config
files from many distributions, stores them in a normalized `data/` tree, and
serves a browser frontend and read-only API for searching Kconfig entries such
as `BPF` or `CONFIG_BPF`.

The repository includes:

- a checked-in `data/` directory with generated package indexes and raw configs
- an HTTP backend and browser frontend that read directly from the local data
- a legacy static site generator for small or offline datasets
- distribution-specific indexers for refreshing or extending the dataset

Supported sources currently include Debian-family distributions, Fedora and
other RPM-based distributions, Arch-family repositories, Alpine, Android GKI,
OpenWrt, Slackware, NixOS, Guix, Void Linux, ChromeOS recovery images, and
more.

> [!CAUTION]
> Most of the code in this repo is created by generative AI agents.
> The indexed kernel configs and result website has been reviewed by human.
> But still, use it at your own risk.

## Use It

The official site is:

- https://kconfigwtf.kxxt.dev/

You can also run it locally from the checked-in `data/` directory:

```sh
cargo run -- serve --data-dir data --listen 127.0.0.1:3000
```

Then open `http://127.0.0.1:3000`. The service loads package indexes at startup,
so restart it after changing `data/`.

The backend exposes cacheable, versioned endpoints:

- `GET /api/v1/configs` — config-name manifest
- `GET /api/v1/configs/<NAME>` — matching kernels
- `GET /api/v1/raw/<PATH>` — a raw config from the local data tree
- `GET /healthz` — health check

API, raw-data, frontend, and error responses have explicit cache policies.
Successful cacheable responses also support `ETag`, `If-None-Match`, and
`HEAD`, and send both standard and CDN-specific cache headers for nginx and
Cloudflare.

## NixOS

The flake provides a package and a NixOS module. A minimal host configuration
looks like this:

```nix
{
  inputs.kconfigwtf.url = "github:kxxt/kconfigwtf";

  outputs = { nixpkgs, kconfigwtf, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        kconfigwtf.nixosModules.default
        {
          services.kconfigwtf = {
            enable = true;
            nginx.virtualHost = "kconfigwtf.example.org";
          };
        }
      ];
    };
  };
}
```

By default the service uses the data bundled in the package. To let the deploy
workflow update data without rebuilding the NixOS system, clone this repository
on the server and point the service at its data tree:

```nix
services.kconfigwtf = {
  enable = true;
  dataDir = "/srv/kconfigwtf/data";
  nginx.virtualHost = "kconfigwtf.example.org";
};
```

The nginx option is deliberately small: it creates a reverse proxy to the
loopback-only backend. TLS and Cloudflare origin settings remain part of your
host configuration. Configure a Cloudflare Cache Rule that makes the desired
`/api/v1/*` and frontend `GET` responses eligible for caching; the origin's
cache lifetimes and validators then control freshness.

## Backend Deployment

Pushes to `main` or `master` run
`.github/workflows/deploy-backend.yml`. It connects to the host, performs a
fast-forward-only pull in the server checkout, and restarts the service to
reload the indexes. Configure these production-environment secrets:

- `KCONFIGWTF_DEPLOY_HOST`
- `KCONFIGWTF_DEPLOY_USER`
- `KCONFIGWTF_DEPLOY_PATH` (for example `/srv/kconfigwtf`)
- `KCONFIGWTF_SSH_PRIVATE_KEY`
- `KCONFIGWTF_SSH_KNOWN_HOSTS`
- `KCONFIGWTF_DEPLOY_PORT` (optional; defaults to `22`)

Install the key's public half in the deploy user's `authorized_keys`. That user
also needs narrowly scoped passwordless permission to run
`systemctl restart kconfigwtf.service`, for example:

```nix
security.sudo.extraRules = [{
  users = [ "kconfigwtf-deploy" ];
  commands = [{
    command = "/run/current-system/sw/bin/systemctl restart kconfigwtf.service";
    options = [ "NOPASSWD" ];
  }];
}];
```

A Git pull updates the mutable data checkout; deploy backend binary/module
changes through the normal NixOS rebuild.

## Refresh The Data

If you want to regenerate part of the dataset, run one of the indexers and then
restart the backend. Example:

```sh
cargo run -- index void \
  --package linux6.6 \
  --arch amd64 \
  --data-dir data

cargo run -- serve --data-dir data
```

Each backend has its own flags and data source expectations.

## Documentation

- Developer guide: [docs/developer-guide.md](./docs/developer-guide.md)
- Indexer design: [docs/indexer.md](./docs/indexer.md)

The developer guide contains the full backend-by-backend indexing instructions,
site generation notes, CI/deployment details, and project architecture.
