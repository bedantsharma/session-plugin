#!/usr/bin/env bash
# One-time developer setup: git hooks, web dependencies, Rust toolchain.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

git config core.hooksPath .githooks
echo "git hooks: enabled (.githooks)"

(cd web && npm ci --silent)
echo "web: dependencies installed"

cargo fetch --quiet
echo "rust: toolchain $(rustc --version) ready"
