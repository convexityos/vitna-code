#!/usr/bin/env bash
# Vitna Code automated release packager (Unix / POSIX shell)
# Packages binaries, computes SHA-256 checksums, and formats release archives.

set -euo pipefail

VERSION="${1:-1.0.0}"
TARGET="${2:-x86_64-unknown-linux-gnu}"
OUTPUT_DIR="${3:-dist}"

echo "==> Packaging Vitna Code v${VERSION} for ${TARGET}"

RELEASE_DIST="${OUTPUT_DIR}/vitna-code-v${VERSION}-${TARGET}"
rm -rf "${RELEASE_DIST}"
mkdir -p "${RELEASE_DIST}/bin"

# Copy compiled binaries (fail fast instead of producing an empty bin/ archive)
TARGET_RELEASE="target/${TARGET}/release"
if [ ! -d "${TARGET_RELEASE}" ]; then
    TARGET_RELEASE="target/release"
fi

for bin in vitna vitna-coded vitna-tui; do
    if [ ! -f "${TARGET_RELEASE}/${bin}" ]; then
        echo "ERROR: missing compiled binary ${TARGET_RELEASE}/${bin}. Run 'cargo build --release' first." >&2
        exit 1
    fi
    cp "${TARGET_RELEASE}/${bin}" "${RELEASE_DIST}/bin/"
done

# Copy metadata and schemas
cp README.md "${RELEASE_DIST}/"
cp CONTRIBUTING.md "${RELEASE_DIST}/"
cp schemas/vitna-run-receipt-v1.json "${RELEASE_DIST}/"
cp schemas/vitna-release-manifest-v1.json "${RELEASE_DIST}/"

# Create tarball
TAR_FILE="${RELEASE_DIST}.tar.gz"
tar -czf "${TAR_FILE}" -C "${OUTPUT_DIR}" "vitna-code-v${VERSION}-${TARGET}"

# Compute SHA-256
if command -v sha256sum >/dev/null 2>&1; then
    HASH=$(sha256sum "${TAR_FILE}" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
    HASH=$(shasum -a 256 "${TAR_FILE}" | awk '{print $1}')
else
    echo "ERROR: neither sha256sum nor shasum is available to hash ${TAR_FILE}" >&2
    exit 1
fi

echo "${HASH}  $(basename "${TAR_FILE}")" >> "${OUTPUT_DIR}/SHA256SUMS"
echo "==> Package created: ${TAR_FILE}"
echo "==> SHA-256: ${HASH}"
