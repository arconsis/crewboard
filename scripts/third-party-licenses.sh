#!/usr/bin/env bash
# Write THIRD-PARTY-LICENSES.md: the license texts and notices of every crate in a release build.
#
#   scripts/third-party-licenses.sh           write the file
#   scripts/third-party-licenses.sh --check   fail when the file in the tree is not current
#
# It needs `cargo-about` (about.toml and about.hbs hold its settings):
#   cargo install --locked --version 0.9.2 --features cli cargo-about
# The file lists the crates of Cargo.lock for the release targets. tests/third_party_licenses.rs
# checks, without cargo-about, that the file and Cargo.lock agree. Run this script after every
# change of Cargo.lock, and commit the result.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/THIRD-PARTY-LICENSES.md"
mode="write"
case "${1:-}" in
  "") ;;
  --check) mode="check" ;;
  *) echo "usage: scripts/third-party-licenses.sh [--check]" >&2; exit 2 ;;
esac

about=${CARGO_ABOUT:-cargo-about}
command -v "$about" >/dev/null 2>&1 || {
  echo "cargo-about is not installed: cargo install --locked --version 0.9.2 --features cli cargo-about" >&2
  exit 3
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$root"

"$about" generate --locked --offline --fail --config about.toml about.hbs -o "$tmp/main.md"

# Crates of Cargo.lock (not the root crate, whose version changes with every release) as "name version", one per line, sorted.
lock_crates() {
  awk '
    /^\[\[package\]\]/ { n = "" }
    /^name = / { gsub(/"/, "", $3); n = $3 }
    /^version = / && n != "" { gsub(/"/, "", $3); if (n != "crewboard") print n " " $3; n = "" }
  ' Cargo.lock | LC_ALL=C sort -u
}
# Crates that the generated list names, as "name version".
listed_crates() {
  sed -n 's/^- \([^ ]*\) \([^ ]*\)\( (.*)\)\{0,1\}$/\1 \2/p' "$tmp/main.md" | LC_ALL=C sort -u
}
lock_crates >"$tmp/lock.txt"
listed_crates >"$tmp/listed.txt"

# The license files of the C code that aws-lc-sys builds into the binary. The generated list only
# holds the license texts by SPDX name. These files hold the copyright notices of each part
# (Amazon, Google, the OpenSSL Project, Eric Young, the fiat-crypto authors and others).
awslc_version=$(awk '$1 == "aws-lc-sys" { print $2 }' "$tmp/listed.txt")
[ -n "$awslc_version" ] || { echo "aws-lc-sys is not in the list of crates" >&2; exit 4; }
cargo_home=${CARGO_HOME:-$HOME/.cargo}
awslc_dir=""
for d in "$cargo_home"/registry/src/*/aws-lc-sys-"$awslc_version"; do
  [ -d "$d" ] && awslc_dir=$d
done
[ -n "$awslc_dir" ] || { echo "source of aws-lc-sys $awslc_version not found under $cargo_home/registry/src" >&2; exit 4; }

{
  cat "$tmp/main.md"
  echo "## C code in aws-lc-sys $awslc_version: complete license files"
  echo
  echo "aws-lc-sys builds the AWS-LC library (C code) into the binary. AWS-LC contains code from"
  echo "BoringSSL, OpenSSL, fiat-crypto, mlkem-native, mldsa-native, s2n-bignum and jitterentropy."
  echo "These are its license files as shipped in the crate."
  echo
  for f in LICENSE aws-lc/LICENSE aws-lc/third_party/fiat/LICENSE; do
    [ -f "$awslc_dir/$f" ] || continue
    echo "### aws-lc-sys: $f"
    echo
    echo '```text'
    cat "$awslc_dir/$f"
    echo '```'
    echo
  done
  echo "## Not part of a release build"
  echo
  echo "These crates are in \`Cargo.lock\` but not in a binary of the release targets: dev"
  echo "dependencies (tests) and crates of other targets."
  echo
  LC_ALL=C comm -23 "$tmp/lock.txt" "$tmp/listed.txt" | awk '{ print "- " $1 " " $2 " (not shipped)" }'
} | tr -d '\r' >"$tmp/final.md"   # some license files have CRLF line ends; git would change them

if [ "$mode" = check ]; then
  if cmp -s "$tmp/final.md" "$out"; then
    echo "THIRD-PARTY-LICENSES.md is current"
  else
    echo "THIRD-PARTY-LICENSES.md is not current: run scripts/third-party-licenses.sh and commit it" >&2
    exit 1
  fi
else
  cp "$tmp/final.md" "$out"
  echo "wrote $out"
fi
