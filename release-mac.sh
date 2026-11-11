#!/usr/bin/env bash
# Builds, packs and uploads the macOS build of a release that release.ps1 already published
# (Windows and Linux), into the same GitHub release. Run it on a Mac:
#
#   ./release-mac.sh 0.2.0            the tag v0.2.0 must exist (release.ps1 creates it)
#   ./release-mac.sh 0.2.0 --local    build and pack into ./releases/osx only, no upload
#
# Needs: Xcode command line tools, Rust with both Mac targets
#   (rustup target add aarch64-apple-darwin x86_64-apple-darwin), .NET and vpk
#   (dotnet tool install -g vpk), and a GitHub token: GITHUB_TOKEN, or `gh auth login`.
#
# Signing (needed before other people can open the app): with an Apple Developer account, set
#   MAC_SIGN_APP="Developer ID Application: Your Name (TEAMID)"
#   MAC_SIGN_INSTALL="Developer ID Installer: Your Name (TEAMID)"
#   MAC_NOTARY_PROFILE=<a profile saved with `xcrun notarytool store-credentials`>
# Without them the build is unsigned: fine on your own Mac, blocked by Gatekeeper elsewhere.
set -euo pipefail

VERSION="${1:?usage: ./release-mac.sh <version> [--local]}"
LOCAL="${2:-}"
REPO=https://github.com/tf200/todo-notes
PACK_ID=TodoNotes
TITLE="Todo & Notes"
# The installed name: no "&" (vpk writes it into XML unescaped).
PACK_TITLE="Todo and Notes"
TAG="v$VERSION"
OUT=releases/osx
STAGE=target/release-stage/osx

cd "$(dirname "$0")"
say() { printf '\033[1;36m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must look like 1.2.3"
command -v vpk >/dev/null || export PATH="$PATH:$HOME/.dotnet/tools"
command -v vpk >/dev/null || die "vpk not found: dotnet tool install -g vpk"

if [ "$LOCAL" != "--local" ]; then
  git fetch -q --tags origin
  git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || die "tag $TAG not found: run release.ps1 first"
  [ -z "$(git status --porcelain)" ] || die "uncommitted changes"
  git checkout -q "$TAG"
  TOKEN="${GITHUB_TOKEN:-$(gh auth token 2>/dev/null || true)}"
  [ -n "$TOKEN" ] || die "no GitHub token: set GITHUB_TOKEN or run gh auth login"
fi

say "Building $TITLE $VERSION (Apple silicon and Intel)"
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
rm -rf "$STAGE" && mkdir -p "$STAGE"
lipo -create -output "$STAGE/todo-notes" \
  target/aarch64-apple-darwin/release/todo-notes target/x86_64-apple-darwin/release/todo-notes

mkdir -p "$OUT"
if [ "$LOCAL" != "--local" ]; then
  say "Fetching the last macOS release"
  vpk download github --repoUrl "$REPO" --token "$TOKEN" --channel osx -o "$OUT" || true
fi

say "Packing"
SIGN=()
if [ -n "${MAC_SIGN_APP:-}" ]; then
  SIGN=(--signAppIdentity "$MAC_SIGN_APP" --signInstallIdentity "${MAC_SIGN_INSTALL:?set MAC_SIGN_INSTALL too}" \
        --notaryProfile "${MAC_NOTARY_PROFILE:?set MAC_NOTARY_PROFILE too}")
else
  say "No signing identity set: the build is unsigned"
fi
vpk pack -u "$PACK_ID" -v "$VERSION" -p "$STAGE" -e todo-notes -o "$OUT" --channel osx \
  -i assets/icon/icon.icns --packTitle "$PACK_TITLE" --packAuthors Taha ${SIGN[@]+"${SIGN[@]}"}

if [ "$LOCAL" = "--local" ]; then
  say "Packed into $OUT (nothing uploaded)"
  exit 0
fi

say "Uploading to $TAG"
vpk upload github -o "$OUT" --channel osx --repoUrl "$REPO" --token "$TOKEN" \
  --tag "$TAG" --releaseName "$TITLE $VERSION" --publish --merge
git checkout -q main
say "macOS added to $REPO/releases/tag/$TAG"
