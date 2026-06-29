# Fedora / RPM packaging

This directory packages PSF Guard as a native RPM that builds with mainline
RPM tooling (`rpmbuild`, `mock`, COPR) on Fedora 43, Fedora 44, and other
recent Fedora releases.

## Design

PSF Guard embeds its React frontend into the binary at compile time
(`include_dir!("static/dist")`) and pulls a large Cargo dependency tree. A
Fedora build environment (mock/COPR) is **offline**, so all network work is
done once, up front, by `scripts/make-rpm-sources.sh`:

1. Export a clean tree from git.
2. Build the frontend (`npm ci && npm run build`) into `static/dist`, then drop
   `node_modules`. At RPM build time `build.rs` is skipped via
   `PSF_GUARD_SKIP_FRONTEND_BUILD=1`.
3. `cargo vendor` every crate into `vendor/`. The spec writes a
   `.cargo/config.toml` pointing at it and builds with `CARGO_NET_OFFLINE=true`.

The result is two sources:

| Source  | File                              | Contents                              |
| ------- | --------------------------------- | ------------------------------------- |
| Source0 | `psf-guard-<ver>.tar.gz`          | source tree + prebuilt `static/dist`  |
| Source1 | `psf-guard-<ver>-vendor.tar.xz`   | vendored crates (`./vendor`)          |

OpenCV is enabled by default (matching the upstream default features). Build a
lighter package without it using `--without opencv`.

## Systemd server service

The package ships a `psf-guard.service` unit that runs the web server
(`psf-guard server`) under a dedicated, unprivileged `psfguard` system user
(created via `sysusers.d`). It is **not** enabled by default.

| Path                          | Purpose                                              |
| ----------------------------- | ---------------------------------------------------- |
| `/etc/psf-guard/server.conf`  | `EnvironmentFile` — host/port, registry, cache, args |
| `/var/lib/psf-guard/`         | `StateDirectory` — the mutable database registry     |
| `/var/cache/psf-guard/`       | `CacheDirectory` — generated/preview image cache     |

```bash
# Configure (bind address, port, extra args) then enable + start:
sudoedit /etc/psf-guard/server.conf
sudo systemctl enable --now psf-guard

# Register a scheduler database + image directories (or do it from the web UI,
# which is enabled by the default --allow-database-management):
sudo -u psfguard psf-guard server \
    --registry /var/lib/psf-guard/registry.json \
    /path/to/schedulerdb.sqlite /path/to/images
sudo systemctl restart psf-guard
```

The unit binds to `127.0.0.1:3000` by default and is sandboxed (`ProtectSystem=strict`,
`ProtectHome=read-only`, restricted syscalls/capabilities). If your image
libraries live somewhere `ProtectHome=read-only` blocks, relax it with a
drop-in: `systemctl edit psf-guard`.

## Build locally

```bash
# Install tooling (Fedora):
sudo dnf install -y rpm-build rpmdevtools cargo rust clang-devel \
    opencv-devel nodejs npm git
rpmdev-setuptree

# Generate the two source tarballs (needs network: npm + cargo vendor).
./scripts/make-rpm-sources.sh                    # -> ~/rpmbuild/SOURCES

# Build (offline from here on).
rpmbuild -ba packaging/rpm/psf-guard.spec

# RPMs land in ~/rpmbuild/RPMS/<arch>/
```

Build without OpenCV:

```bash
rpmbuild -ba --without opencv packaging/rpm/psf-guard.spec
```

## Build in clean Fedora containers (podman)

`build-in-podman.sh` reproduces the CI build locally: it runs the whole flow
(toolchain install, source generation, `rpmbuild`, and an `rpm -i` +
`psf-guard --help` smoke test) inside throwaway `fedora:<ver>` containers and
drops the artifacts on the host.

```bash
# Builds Fedora 43 and 44 in parallel into ./dist/rpm/fedora-<ver>/
./packaging/rpm/build-in-podman.sh

# Pick releases and an output directory; build one at a time:
./packaging/rpm/build-in-podman.sh 43 44 --outdir /tmp/psf-rpms --sequential
```

Each release lands in `<outdir>/fedora-<ver>/` alongside a `build.log`. Only
podman is required on the host (the container does the rest; network is needed
for npm + cargo vendor).

## Build in mock (clean chroot, e.g. Fedora 44)

```bash
./scripts/make-rpm-sources.sh --outdir /tmp/psf-sources
rpmbuild -bs packaging/rpm/psf-guard.spec \
    --define "_sourcedir /tmp/psf-sources"
mock -r fedora-44-x86_64 ~/rpmbuild/SRPMS/psf-guard-*.src.rpm
```

## Releasing a new version

1. Bump `Version:` in `psf-guard.spec` to match `Cargo.toml`.
2. Add a `%changelog` entry.
3. Regenerate sources for the tag: `./scripts/make-rpm-sources.sh --ref vX.Y.Z`.

CI (`.github/workflows/rpm.yml`) builds the RPMs in Fedora 43 and 44 containers
on every push and pull request and uploads them as artifacts.
