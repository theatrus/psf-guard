#!/usr/bin/env bash
#
# make-rpm-sources.sh - Produce the source tarballs consumed by
# packaging/rpm/psf-guard.spec.
#
# The Fedora spec is built fully offline (mock/COPR friendly), so every step
# that needs the network happens here, once, at source-prep time:
#
#   1. Export a clean tree from git (no working-tree cruft).
#   2. Build the embedded React frontend (npm ci && npm run build) so that
#      `static/dist` exists -- build.rs is then skipped at rpm build time via
#      PSF_GUARD_SKIP_FRONTEND_BUILD=1.
#   3. Vendor all Cargo dependencies (`cargo vendor`) so the offline build has
#      every crate locally.
#
# Outputs two sources into --outdir (default: ~/rpmbuild/SOURCES):
#   psf-guard-<version>.tar.gz         (Source0: source + prebuilt static/dist)
#   psf-guard-<version>-vendor.tar.xz  (Source1: vendored crates -> ./vendor)
#
# Usage:
#   scripts/make-rpm-sources.sh [--ref <git-ref>] [--outdir <dir>] [--version <v>]
#
# Defaults: ref=HEAD, version read from Cargo.toml, outdir=~/rpmbuild/SOURCES.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GIT_REF="HEAD"
OUTDIR="${HOME}/rpmbuild/SOURCES"
VERSION=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --ref) GIT_REF="$2"; shift 2 ;;
        --outdir) OUTDIR="$2"; shift 2 ;;
        --version) VERSION="$2"; shift 2 ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

cd "$REPO_ROOT"

if [[ -z "$VERSION" ]]; then
    VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
fi
if [[ -z "$VERSION" ]]; then
    echo "could not determine version" >&2
    exit 1
fi

NAME="psf-guard"
PREFIX="${NAME}-${VERSION}"

for tool in git npm cargo tar xz; do
    command -v "$tool" >/dev/null 2>&1 || { echo "missing required tool: $tool" >&2; exit 1; }
done

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo ">> Exporting ${GIT_REF} -> ${PREFIX}/"
git archive --format=tar --prefix="${PREFIX}/" "$GIT_REF" | tar -x -C "$WORK"
SRC="${WORK}/${PREFIX}"

echo ">> Building frontend (npm ci && npm run build)"
(
    cd "${SRC}/static"
    npm ci
    npm run build
    # node_modules is huge and not needed in the source tarball; the dist
    # output is what gets embedded into the binary.
    rm -rf node_modules
)

if [[ ! -d "${SRC}/static/dist" ]]; then
    echo "frontend build did not produce static/dist" >&2
    exit 1
fi

mkdir -p "$OUTDIR"

echo ">> Writing Source0: ${PREFIX}.tar.gz"
tar -C "$WORK" --owner=0 --group=0 --numeric-owner \
    -czf "${OUTDIR}/${PREFIX}.tar.gz" "${PREFIX}"

echo ">> Vendoring Cargo dependencies"
(
    cd "$SRC"
    # Vendor into ./vendor; spec extracts this at the source root and points
    # .cargo/config.toml at it. Suppress the config snippet on stdout.
    cargo vendor --locked vendor >/dev/null
)

echo ">> Writing Source1: ${PREFIX}-vendor.tar.xz"
tar -C "$SRC" --owner=0 --group=0 --numeric-owner \
    -cJf "${OUTDIR}/${PREFIX}-vendor.tar.xz" vendor

echo
echo "Done. Sources in ${OUTDIR}:"
echo "  ${PREFIX}.tar.gz"
echo "  ${PREFIX}-vendor.tar.xz"
