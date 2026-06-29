#!/usr/bin/env bash
#
# build-in-podman.sh - Build the PSF Guard RPM in clean Fedora containers.
#
# Mirrors .github/workflows/rpm.yml locally: for each requested Fedora release
# it spins up a fedora:<ver> container, installs the toolchain, runs
# scripts/make-rpm-sources.sh (frontend build + cargo vendor), builds the RPM
# with rpmbuild, smoke-tests it (rpm -i + `psf-guard --help`), and copies the
# resulting RPMs + SRPM out to the host.
#
# Usage:
#   packaging/rpm/build-in-podman.sh [VERSION...] [--outdir DIR] [--sequential]
#
#   VERSION...     Fedora releases to build (default: 43 44)
#   --outdir DIR   Where to drop artifacts (default: <repo>/dist/rpm)
#                  Each release lands in DIR/fedora-<ver>/ with a build.log.
#   --sequential   Build one release at a time (default: all in parallel)
#
# Requires: podman. Needs network (npm + cargo vendor run inside the container).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUTDIR="${REPO_ROOT}/dist/rpm"
SEQUENTIAL=0
VERSIONS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --outdir) OUTDIR="$2"; shift 2 ;;
        --sequential) SEQUENTIAL=1; shift ;;
        -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) VERSIONS+=("$1"); shift ;;
    esac
done

if [[ ${#VERSIONS[@]} -eq 0 ]]; then
    VERSIONS=(43 44)
fi

command -v podman >/dev/null 2>&1 || { echo "podman is required" >&2; exit 1; }

# The script executed inside each container.
read -r -d '' CONTAINER_SCRIPT <<'EOS' || true
set -euxo pipefail
dnf -y install --setopt=install_weak_deps=False \
    rpm-build rpmdevtools \
    rust cargo clang-devel opencv-devel \
    gcc gcc-c++ pkgconf-pkg-config \
    nodejs npm git xz tar findutils
rpmdev-setuptree
git config --global --add safe.directory /src
cd /src
./scripts/make-rpm-sources.sh
cp packaging/rpm/psf-guard.spec ~/rpmbuild/SPECS/
rpmbuild -ba ~/rpmbuild/SPECS/psf-guard.spec
mkdir -p /out
find ~/rpmbuild/RPMS ~/rpmbuild/SRPMS -name '*.rpm' -exec cp {} /out/ \;
# Smoke test: install the binary RPM (dnf resolves Requires, incl. systemd)
# and confirm the binary, unit, and config landed.
dnf -y install /out/psf-guard-[0-9]*.x86_64.rpm
psf-guard --help | head -5
test -f /usr/lib/systemd/system/psf-guard.service
test -f /etc/psf-guard/server.conf
systemd-analyze verify /usr/lib/systemd/system/psf-guard.service || true
echo "BUILD_OK"
EOS

build_one() {
    local ver="$1"
    local dest="${OUTDIR}/fedora-${ver}"
    mkdir -p "$dest"
    echo ">> Building for Fedora ${ver} -> ${dest}"
    if podman run --rm -i --security-opt label=disable \
        -v "${REPO_ROOT}":/src:ro \
        -v "${dest}":/out \
        "fedora:${ver}" bash -s <<<"$CONTAINER_SCRIPT" >"${dest}/build.log" 2>&1
    then
        if grep -q BUILD_OK "${dest}/build.log"; then
            echo ">> Fedora ${ver}: OK"
            return 0
        fi
    fi
    echo ">> Fedora ${ver}: FAILED (see ${dest}/build.log)" >&2
    return 1
}

rc=0
if [[ $SEQUENTIAL -eq 1 ]]; then
    for ver in "${VERSIONS[@]}"; do
        build_one "$ver" || rc=1
    done
else
    declare -A pids=()
    for ver in "${VERSIONS[@]}"; do
        build_one "$ver" &
        pids[$ver]=$!
    done
    for ver in "${VERSIONS[@]}"; do
        wait "${pids[$ver]}" || rc=1
    done
fi

echo
echo "=== Artifacts ==="
find "$OUTDIR" -name '*.rpm' -printf '%p (%s bytes)\n' | sort || true
exit $rc
