# Build offline from vendored crates; the frontend dist ships prebuilt in
# Source0 (see scripts/make-rpm-sources.sh), so no network or npm is needed
# here -- this builds cleanly in mock/COPR.

# Rust release binaries carry no DWARF here (the release profile sets no
# debug), so there is nothing for find-debuginfo to harvest.
%global debug_package %{nil}

Name:           psf-guard
Version:        0.6.6
Release:        1%{?dist}
Summary:        Astronomical image analysis and quality assessment tool for N.I.N.A.

License:        Apache-2.0
URL:            https://github.com/theatrus/psf-guard

# Both sources are produced by scripts/make-rpm-sources.sh (the upstream git
# archive does not contain the prebuilt frontend or the vendored crates).
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.xz

BuildRequires:  rust >= 1.89.0
BuildRequires:  cargo
# libsqlite3-sys is built with the `bundled` feature: it compiles SQLite from C.
BuildRequires:  gcc
# systemd unit + sysusers.d handling (%%_unitdir, %%systemd_* macros, etc.).
BuildRequires:  systemd-rpm-macros

# Pulls the right Requires for the sysusers.d-created system user.
%{?sysusers_requires_compat}
Requires(post):   systemd
Requires(preun):  systemd
Requires(postun): systemd
# SQLite is statically bundled, so it is not a runtime dependency; image
# processing is pure Rust (seiza-imgproc), so no OpenCV is required.

%description
PSF Guard is a command-line utility and web server for analyzing N.I.N.A.
Target Scheduler databases and managing astronomical image files. It ports the
N.I.N.A. star-detection algorithm, performs PSF fitting (Gaussian/Moffat), and
serves an embedded React web interface for reviewing, grading, and organizing
FITS captures.

%prep
%autosetup -n %{name}-%{version} -p1
# rust-toolchain.toml pins a rustup channel; Fedora's cargo ignores it, but
# remove it so no rustup-based environment tries to fetch a toolchain offline.
rm -f rust-toolchain.toml
# Drop the vendored crates and cargo vendor's complete source-replacement
# config in beside the source. The generated config covers crates.io and any
# pinned Git sources without allowing network access during the RPM build.
tar -xf %{SOURCE1}

%build
# The frontend is already built into static/dist by make-rpm-sources.sh; tell
# build.rs not to invoke npm. Crates resolve from ./vendor, so build offline.
export PSF_GUARD_SKIP_FRONTEND_BUILD=1
export CARGO_NET_OFFLINE=true
cargo build --release --locked

%install
install -Dpm0755 target/release/%{name} %{buildroot}%{_bindir}/%{name}

# systemd server integration.
install -Dpm0644 packaging/rpm/systemd/psf-guard.service \
    %{buildroot}%{_unitdir}/%{name}.service
install -Dpm0644 packaging/rpm/systemd/psf-guard.sysusers \
    %{buildroot}%{_sysusersdir}/%{name}.conf
install -Dpm0644 packaging/rpm/systemd/psf-guard-server.conf \
    %{buildroot}%{_sysconfdir}/%{name}/server.conf

# The psfguard system user is created by the sysusers.d file trigger that
# systemd installs for %%{_sysusersdir}, so no %%pre useradd is needed.
%post
%systemd_post %{name}.service

%preun
%systemd_preun %{name}.service

%postun
%systemd_postun_with_restart %{name}.service

%files
%license LICENSE
%doc README.md psf-guard.toml.example
%{_bindir}/%{name}
%{_unitdir}/%{name}.service
%{_sysusersdir}/%{name}.conf
%dir %{_sysconfdir}/%{name}
%config(noreplace) %{_sysconfdir}/%{name}/server.conf

%changelog
* Tue Jul 28 2026 Yann Ramin <github@theatr.us> - 0.6.6-1
- Notarize and staple the macOS disk image for Gatekeeper

* Tue Jul 28 2026 Yann Ramin <github@theatr.us> - 0.6.5-1
- Fix N.I.N.A. pixel-scale hints and install star identifier catalogs
- Show the release summary in update notices

* Sun Jul 26 2026 Yann Ramin <github@theatr.us> - 0.6.4-1
- Split Settings into Databases, Catalogs, and Sync tabs

* Sun Jul 26 2026 Yann Ramin <github@theatr.us> - 0.6.3-1
- Offer both catalog import paths on first run

* Sun Jul 26 2026 Yann Ramin <github@theatr.us> - 0.6.2-1
- Label the Apple Silicon standalone CLI with its correct architecture

* Sun Jul 26 2026 Yann Ramin <github@theatr.us> - 0.6.1-1
- Fix Windows release packaging and publish the 0.6 feature set

* Sun Jul 26 2026 Yann Ramin <github@theatr.us> - 0.6.0-1
- Add FITS catalog import and export, stack and color previews, calibration
  libraries, satellite trail checks, remote sync, and signed desktop updates

* Sun Jul 19 2026 Yann Ramin <github@theatr.us> - 0.5.0-1
- Add Seiza plate solving, astrometry overlays and evidence, off-target
  sequence grading, and astrometry-aware regrading

* Sun Jul 12 2026 Yann Ramin <github@theatr.us> - 0.4.2-1
- Update to 0.4.2: FITS reading, header parsing, and image statistics now go
  through seiza-fits, substantially speeding up every FITS-backed operation
  (screening, previews, star detection)

* Mon Jul 06 2026 Yann Ramin <github@theatr.us> - 0.4.1-1
- Update to 0.4.1: detail-view image rendering fixes while navigating and
  zooming, restored NSIS release uploads, release/CI reliability improvements,
  README screenshot refreshes, and dependency updates

* Sun Jul 05 2026 Yann Ramin <github@theatr.us> - 0.4.0-1
- Update to 0.4.0: priority-aware worker pools, async on-demand preview
  generation, occlusion/cloud photometric screening improvements, two-database
  sync commands, out-of-tree reject archive, dedicated psf-guard-cli binary,
  and a working server --host flag

* Sun Jun 28 2026 Yann Ramin <github@theatr.us> - 0.3.0-2
- Bump packaging release

* Sun Jun 28 2026 Yann Ramin <github@theatr.us> - 0.3.0-1
- Initial Fedora packaging (offline build from vendored crates + prebuilt frontend)
- Ship psf-guard.service (server mode) with a dedicated psfguard system user,
  StateDirectory registry, CacheDirectory cache, and /etc/psf-guard/server.conf
