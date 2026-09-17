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

# Copy binaries if present
TARGET_RELEASE="target/${TARGET}/release"
if [ -d "${TARGET_RELEASE}" ]; then
    cp "${TARGET_RELEASE}/vitna" "${RELEASE_DIST}/bin/" 2>/dev/null || true
    cp "${TARGET_RELEASE}/vitna-tui" "${RELEASE_DIST}/bin/" 2>/dev/null || true
fi

# Copy metadata and schemas
cp README.md "${RELEASE_DIST}/"
cp CONTRIBUTING.md "${RELEASE_DIST}/"
cp schemas/vitna-run-receipt-v1.json "${RELEASE_DIST}/"

# Create tarball
TAR_FILE="${RELEASE_DIST}.tar.gz"
tar -czf "${TAR_FILE}" -C "${OUTPUT_DIR}" "vitna-code-v${VERSION}-${TARGET}"

# Compute SHA-256
if command -v sha256sum >/dev/null 2>&1; then
    HASH=$(sha256sum "${TAR_FILE}" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
    HASH=$(shasum -a 256 "${TAR_FILE}" | awk '{print $1}')
else
    HASH="unknown"
fi

echo "${HASH}  $(basename "${TAR_FILE}")" >> "${OUTPUT_DIR}/SHA256SUMS"
echo "==> Package created: ${TAR_FILE}"
echo "==> SHA-256: ${HASH}"
