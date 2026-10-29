#!/usr/bin/env bash
# release-macos.sh - add the two macOS tarballs to an existing crewboard release.
#
# Usage: scripts/maintainer/release-macos.sh [--force] <vX.Y.Z>
#
# For maintainers only. Use it when the release has no macOS assets, for example when the macOS
# build of the workflow did not run. Run it on a Mac, in a clone of the repo, with `gh` logged in
# and write access to the repo.
# The release must exist (a draft is fine): the release workflow creates it from the Linux
# tarballs. The script builds the tag with the stable toolchain in a temporary worktree for
# aarch64-apple-darwin and x86_64-apple-darwin, checks and packages them as build.yml does,
# keeps the Linux lines of SHA256SUMS, adds the two darwin lines, checks every line against the
# assets, and uploads both tarballs and the new SHA256SUMS. A draft release is then published.
# It refuses to replace darwin assets that the release already has, unless you give --force.
# Environment: REPO (default arconsis/crewboard).
set -euo pipefail

REPO=${REPO:-arconsis/crewboard}
die() { echo "release-macos.sh: $*" >&2; exit 1; }

FORCE=0
if [ "${1:-}" = "--force" ]; then FORCE=1; shift; fi
[ $# -eq 1 ] || die "usage: scripts/maintainer/release-macos.sh [--force] <vX.Y.Z>"
TAG=$1
printf '%s\n' "$TAG" | grep -Eqx 'v[0-9]+\.[0-9]+\.[0-9]+' || die "tag '$TAG' is not vX.Y.Z"
VERSION=${TAG#v}
[ "$(uname -s)" = Darwin ] || die "run this on a Mac"
for c in gh git cargo rustup lipo file shasum tar; do command -v "$c" >/dev/null || die "$c not found"; done
[ -z "${GH_TOKEN:-}${GITHUB_TOKEN:-}" ] || die "unset GH_TOKEN and GITHUB_TOKEN: gh must use your login"

gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1 || die "release $TAG does not exist in $REPO (the release workflow creates it)"
is_draft=$(gh release view "$TAG" --repo "$REPO" --json isDraft --jq .isDraft)
if [ "$FORCE" -eq 0 ] && gh release view "$TAG" --repo "$REPO" --json assets --jq '.assets[].name' | grep -q 'apple-darwin'; then
  die "release $TAG already has macOS assets; use --force to replace them"
fi
repo_root=$(git rev-parse --show-toplevel) || die "run this inside a clone of the repo"
git -C "$repo_root" fetch --force origin "refs/tags/$TAG:refs/tags/$TAG" || die "cannot fetch tag $TAG"

T=$(mktemp -d "${TMPDIR:-/tmp}/crewboard-release-macos.XXXXXX")
cleanup() {
  git -C "$repo_root" worktree remove --force "$T/src" >/dev/null 2>&1 || true
  git -C "$repo_root" worktree prune >/dev/null 2>&1 || true
  rm -rf "$T"
}
trap cleanup EXIT

git -C "$repo_root" worktree add --detach "$T/src" "refs/tags/$TAG" >/dev/null
cd "$T/src"
got=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
[ "$got" = "$VERSION" ] || die "Cargo.toml version is '$got', the tag says '$VERSION'"
commit=$(git rev-parse HEAD)
# The tag of the clone's origin must be the tag of the release repo (origin may be a fork).
remote=$(gh api "repos/$REPO/commits/refs/tags/$TAG" --jq .sha) || die "cannot read the tag $TAG of $REPO"
[ "$remote" = "$commit" ] || die "the tag $TAG is at $commit here, but at $remote in $REPO"

export CARGO_TARGET_DIR="$T/target"
mkdir -p "$T/dist"
host=$(uname -m)   # arm64 or x86_64
for spec in aarch64-apple-darwin:arm64 x86_64-apple-darwin:x86_64; do
  target=${spec%%:*}; arch=${spec##*:}
  echo "== $target"
  rustup toolchain install stable --profile minimal --target "$target"
  rustc +stable -V
  cargo +stable build --release --locked --target "$target"
  bin="$CARGO_TARGET_DIR/$target/release/crewboard"
  file "$bin" | grep -q "$arch" || die "$bin is not a $arch binary"
  lipo -archs "$bin" | grep -qx "$arch" || die "lipo says '$(lipo -archs "$bin")', expected $arch"
  out=""
  if [ "$host" = "$arch" ]; then
    out=$("$bin" --version)
  elif [ "$host" = arm64 ] && /usr/bin/arch -x86_64 /usr/bin/true 2>/dev/null; then
    out=$(/usr/bin/arch -x86_64 "$bin" --version)   # Rosetta
  else
    echo "host is $host: the binary of $arch is not run"
  fi
  if [ -n "$out" ]; then
    echo "$out"
    [ "$out" = "crewboard $VERSION" ] || die "'$bin --version' prints '$out', expected 'crewboard $VERSION'"
  fi
  name="crewboard-$VERSION-$target"
  mkdir -p "$T/dist/$name"
  cp "$bin" "$T/dist/$name/"
  for f in README.md LICENSE NOTICE THIRD-PARTY-LICENSES.md LICENSE.md LICENSE-MIT LICENSE-APACHE; do
    if [ -f "$f" ]; then cp "$f" "$T/dist/$name/"; fi
  done
  tar -C "$T/dist" -czf "$T/dist/$name.tar.gz" "$name"
  tar -tzf "$T/dist/$name.tar.gz"
done

# The release's other tarballs must be of the same version (the same tag).
mkdir -p "$T/check"
gh release download "$TAG" --repo "$REPO" --dir "$T/check" --pattern SHA256SUMS || die "release $TAG has no SHA256SUMS"
[ -s "$T/check/SHA256SUMS" ] || die "SHA256SUMS of $TAG is empty"
while read -r _ f; do
  case "$f" in
    "crewboard-$VERSION-"*.tar.gz) ;;
    *) die "SHA256SUMS of $TAG lists '$f', not a tarball of version $VERSION" ;;
  esac
done < "$T/check/SHA256SUMS"

# New SHA256SUMS: the Linux lines of the release, then the two darwin lines.
{ grep -v -E ' crewboard-[^ ]*-apple-darwin\.tar\.gz$' "$T/check/SHA256SUMS" || true; } > "$T/check/SHA256SUMS.new"
( cd "$T/dist" && shasum -a 256 crewboard-*-apple-darwin.tar.gz ) >> "$T/check/SHA256SUMS.new"
sort -k2 "$T/check/SHA256SUMS.new" > "$T/dist/SHA256SUMS"
cat "$T/dist/SHA256SUMS"
[ "$(grep -c 'apple-darwin' "$T/dist/SHA256SUMS")" -eq 2 ] || die "SHA256SUMS must list exactly the 2 darwin tarballs"
# Check every line of the new SHA256SUMS against the assets: the Linux tarballs come from the
# release, the darwin ones from this build.
rm -f "$T/check/SHA256SUMS.new"
gh release download "$TAG" --repo "$REPO" --dir "$T/check" --clobber --pattern 'crewboard-*-linux-gnu.tar.gz'
cp "$T"/dist/crewboard-*-apple-darwin.tar.gz "$T/dist/SHA256SUMS" "$T/check/"
( cd "$T/check" && shasum -a 256 -c SHA256SUMS )

( cd "$T/dist" && gh release upload "$TAG" --repo "$REPO" --clobber \
    "crewboard-$VERSION-aarch64-apple-darwin.tar.gz" "crewboard-$VERSION-x86_64-apple-darwin.tar.gz" SHA256SUMS )
if [ "$is_draft" = true ]; then
  gh release edit "$TAG" --repo "$REPO" --draft=false --latest
  echo "published $TAG"
fi
echo "built from $commit"
gh release view "$TAG" --repo "$REPO" --json assets,isDraft --jq '{draft: .isDraft, assets: [.assets[].name]}'
