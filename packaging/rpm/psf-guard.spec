# Build offline from vendored crates; the frontend dist ships prebuilt in
# Source0 (see scripts/make-rpm-sources.sh), so no network or npm is needed
# here -- this builds cleanly in mock/COPR.
#
# OpenCV support is on by default (matches the upstream default feature set).
# Build a lighter package without it:  rpmbuild --without opencv ...
%bcond_without opencv

# Rust release binaries carry no DWARF here (the release profile sets no
# debug), so there is nothing for find-debuginfo to harvest.
%global debug_package %{nil}

Name:           psf-guard
Version:        0.3.0
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
%if %{with opencv}
# The opencv crate runs bindgen (needs libclang), compiles a C++ shim, links
# the system OpenCV, and probes it via pkg-config in build.rs.
BuildRequires:  clang-devel
BuildRequires:  gcc-c++
BuildRequires:  opencv-devel
# build.rs shells out to the pkg-config binary to detect the OpenCV version.
BuildRequires:  pkgconfig
%endif

# Runtime dependencies are resolved automatically by RPM's ELF dependency
# generator (the OpenCV sonames the binary links against). SQLite is statically
# bundled, so it is not a runtime dependency.

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
# Drop the vendored crates in beside the source and point cargo at them.
tar -xf %{SOURCE1}
mkdir -p .cargo
cat > .cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
# The frontend is already built into static/dist by make-rpm-sources.sh; tell
# build.rs not to invoke npm. Crates resolve from ./vendor, so build offline.
export PSF_GUARD_SKIP_FRONTEND_BUILD=1
export CARGO_NET_OFFLINE=true
cargo build --release --locked \
%if %{without opencv}
    --no-default-features \
%endif
    %{nil}

%install
install -Dpm0755 target/release/%{name} %{buildroot}%{_bindir}/%{name}

%files
%license LICENSE
%doc README.md psf-guard.toml.example
%{_bindir}/%{name}

%changelog
* Sun Jun 28 2026 Yann Ramin <github@theatr.us> - 0.3.0-1
- Initial Fedora packaging (offline build from vendored crates + prebuilt frontend)
