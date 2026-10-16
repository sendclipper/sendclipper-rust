#!/usr/bin/env bash
# Publish sdks/rust to its public GitHub repository.
#
# The monorepo (GitLab, private) is the source of truth. This pushes ONLY the
# sdks/rust subtree, with its history, to github.com/sendclipper/sendclipper-rust.
# Never push the monorepo itself there.
#
#   sdks/rust/scripts/split-to-github.sh            # push main
#   sdks/rust/scripts/split-to-github.sh v0.1.0     # push main and tag v0.1.0 (publishes to crates.io)
#
# A crates.io version can never be republished (only yanked), so let CI go
# green on main first, then tag.
#
# REMOTE defaults to the `github-sendclipper` SSH alias (~/.ssh/config), which
# authenticates as the sendclipper GitHub account.
set -euo pipefail

REMOTE="${REMOTE:-git@github-sendclipper:sendclipper/sendclipper-rust.git}"
TAG="${1:-}"
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

if [ -n "$(git status --porcelain -- sdks/rust)" ]; then
  echo "sdks/rust has uncommitted changes; commit them first." >&2
  exit 1
fi

if [ -n "$TAG" ]; then
  version="$(sed -n 's/^version = "\(.*\)"/\1/p' sdks/rust/Cargo.toml | head -1)"
  if [ "v$version" != "$TAG" ]; then
    echo "tag $TAG does not match Cargo.toml version $version" >&2
    exit 1
  fi
fi

SPLIT="$(git subtree split --prefix=sdks/rust HEAD 2>/dev/null)"
echo "split commit: $SPLIT"
git push "$REMOTE" "$SPLIT:refs/heads/main"

if [ -n "$TAG" ]; then
  git push "$REMOTE" "$SPLIT:refs/tags/$TAG"
fi
