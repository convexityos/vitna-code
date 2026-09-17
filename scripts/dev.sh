#!/usr/bin/env bash
# POSIX development helper script for Vitna Code
set -euo pipefail

ACTION="${1:-check}"

case "$ACTION" in
    check)
        cargo check --workspace --all-targets
        ;;
    test)
        cargo test --workspace
        ;;
    lint)
        cargo clippy --workspace --all-targets -- -D warnings
        ;;
    fmt)
        cargo fmt --all
        ;;
    scan)
        echo "Scanning for forbidden em-dash characters..."
        if grep -rn --exclude-dir=".git" -P "\x{2014}" .; then
            echo "ERROR: Em-dash found in files!"
            exit 1
        fi
        echo "Em-dash check clean."
        ;;
    *)
        echo "Usage: $0 {check|test|lint|fmt|scan}"
        exit 1
        ;;
esac
