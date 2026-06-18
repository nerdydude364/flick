#!/usr/bin/env bash
# Cuts a release: tags the current commit as `vX.Y.Z`, matching Cargo.toml's
# `version` (the single source of truth), and pushes the tag.
#
# That push is what triggers .github/workflows/release.yml, which builds the
# macOS/Linux/Windows x amd64/arm64 matrix and attaches the artifacts to a new
# GitHub release. Bump Cargo.toml's version and commit it before running this.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -n "$(git status --porcelain)" ]; then
  echo "Working tree isn't clean — commit or stash changes first." >&2
  exit 1
fi

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/version = "(.*)"/\1/')"
TAG="v$VERSION"

if git rev-parse "$TAG" >/dev/null 2>&1; then
  echo "Tag $TAG already exists — bump Cargo.toml's version first." >&2
  exit 1
fi

echo "==> Tagging $TAG"
git tag -a "$TAG" -m "Release $TAG"

echo "==> Pushing $TAG"
git push origin "$TAG"

echo "==> Pushed $TAG — the Release workflow will pick it up from here."
