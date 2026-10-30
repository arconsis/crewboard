#!/bin/bash
# install.sh - install a prebuilt crewboard release: the binary, which carries the skill.
#
#   curl -fsSL https://raw.githubusercontent.com/arconsis/crewboard/main/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/arconsis/crewboard/main/install.sh | bash -s -- --version v0.8.0
#
# Options (after `bash -s --` when the script comes from a pipe):
#   --version  the release tag (default: latest). "0.8.0" and "v0.8.0" are the same.
#   --prefix   the binary goes to <prefix>/bin/crewboard (default: ~/.local)
#   --runtime  claude, codex, copilot or hermes. Link the skill into that runtime even when it is
#              not found on this host. Repeat it for more runtimes. Without it, the skill is
#              linked into every runtime that is present (`crewboard skill install` decides).
#   --dry-run  download and verify the release in a temporary directory, print the plan of
#              `crewboard skill install`, and change nothing else
#
# Steps: find the OS and CPU, download the binary tarball and SHA256SUMS from the GitHub release
# with curl, verify the checksum, install the binary to <prefix>/bin, then run
# `crewboard skill install` with the new binary (the skill files are inside it). Nothing is
# installed if a check fails. Run it again for an upgrade (or use `crewboard update`): it is
# idempotent. No root. bash 3.2 compatible.
#
# If curl gets a 404 (for example on a private fork or mirror), the script uses
# `gh release download` with your `gh` login, which needs read access to the repo.
#
# Upgrade: stop the loops of this host first (docs/guides/upgrade.md). The install replaces the
# binary that a running wake calls and the skill files that a running agent reads.
#
# The whole script is one function that runs at the end, so bash has read all of it before the
# first command runs: this is what makes `curl ... | bash` safe.

REPO="${CREWBOARD_RELEASE_REPO:-arconsis/crewboard}"
WEB="${CREWBOARD_RELEASE_WEB:-https://github.com}"

die() { echo "error: $*" >&2; exit 1; }

usage() {
  cat <<'USAGE'
Usage: install.sh [--version vX.Y.Z|latest] [--prefix <dir>] [--runtime <name>]... [--dry-run]
  --version  the release tag (default: latest). "0.8.0" and "v0.8.0" are the same.
  --prefix   the binary goes to <prefix>/bin/crewboard (default: ~/.local)
  --runtime  claude, codex, copilot or hermes: link the skill into that runtime even when it is
             not found on this host (repeatable). Without it, every runtime that is present
             gets the link.
  --dry-run  download and verify the release, print the plan, change nothing else
From a pipe: curl -fsSL https://raw.githubusercontent.com/arconsis/crewboard/main/install.sh | bash -s -- --version v0.8.0
USAGE
}

main() {
  # A child process must never read the script from the pipe.
  exec </dev/null

  VERSION=latest
  PREFIX="$HOME/.local"
  DRY=0
  RUNTIMES=()

  while [ $# -gt 0 ]; do
    case "$1" in
      --version)
        [ $# -ge 2 ] || { echo "--version needs a tag, for example v0.8.0, or latest" >&2; exit 2; }
        VERSION="$2"; shift ;;
      --prefix)
        [ $# -ge 2 ] || { echo "--prefix needs a directory" >&2; exit 2; }
        PREFIX="$2"; shift ;;
      --runtime)
        [ $# -ge 2 ] || { echo "--runtime needs a name: claude, codex, copilot or hermes" >&2; exit 2; }
        RUNTIMES+=(--runtime "$2"); shift ;;
      --dry-run) DRY=1 ;;
      -h|--help) usage; exit 0 ;;
      *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
  done

  case "$VERSION" in
    latest) ;;
    v*) ;;
    *) VERSION="v$VERSION" ;;
  esac
  if [ "$VERSION" != latest ] && ! printf '%s\n' "$VERSION" | grep -Eqx 'v[0-9]+\.[0-9]+\.[0-9]+'; then
    echo "--version must be vX.Y.Z or latest, not '$VERSION'" >&2; exit 2
  fi

  # 1. The target: OS and CPU. Only the targets that the release workflow builds.
  os=$(uname -s 2>/dev/null)
  cpu=$(uname -m 2>/dev/null)
  # A shell under Rosetta 2 reports x86_64 on an Apple silicon Mac. Use the native binary there.
  if [ "$os" = Darwin ] && [ "$cpu" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
    cpu=arm64
  fi
  case "$os/$cpu" in
    Darwin/arm64|Darwin/aarch64) TARGET=aarch64-apple-darwin ;;
    Darwin/x86_64)               TARGET=x86_64-apple-darwin ;;
    Linux/x86_64|Linux/amd64)    TARGET=x86_64-unknown-linux-gnu ;;
    Linux/aarch64|Linux/arm64)   TARGET=aarch64-unknown-linux-gnu ;;
    *)
      die "no prebuilt binary for $os/$cpu. Prebuilt binaries exist for macOS (arm64, x86_64) and Linux (x86_64, arm64). Build from source instead: docs/guides/install.md, section \"Build from source\"." ;;
  esac

  # 2. The tag. First with curl on the public release pages; then, if that fails (for example
  # a private fork or mirror), with gh.
  MODE=curl
  TAG=""
  if command -v curl >/dev/null 2>&1; then
    if [ "$VERSION" = latest ]; then page="$WEB/$REPO/releases/latest"; else page="$WEB/$REPO/releases/tag/$VERSION"; fi
    final=$(curl -fsSL -o /dev/null -w '%{url_effective}' "$page" 2>/dev/null) || final=""
    case "$final" in */releases/tag/v*) TAG=${final##*/releases/tag/}; TAG=${TAG%%[/?#]*} ;; esac
  fi
  if [ -z "$TAG" ]; then
    MODE=gh
    gh_tag
  fi
  printf '%s\n' "$TAG" | grep -Eqx 'v[0-9]+\.[0-9]+\.[0-9]+' || die "unexpected release tag '$TAG'"
  VER=${TAG#v}
  BIN_TGZ="crewboard-$VER-$TARGET.tar.gz"
  echo "release: $REPO $TAG   target: $TARGET   prefix: $PREFIX$([ "$DRY" -eq 1 ] && echo '   (dry run)')"

  # 3. Download into a temporary directory, which is removed at the end.
  T=$(mktemp -d "${TMPDIR:-/tmp}/crewboard-release.XXXXXX") || die "cannot make a temporary directory"
  trap 'rm -rf "$T"' EXIT
  mkdir -p "$T/dl"
  if [ "$MODE" = curl ]; then
    for f in "$BIN_TGZ" SHA256SUMS; do
      curl -fsSL -o "$T/dl/$f" "$WEB/$REPO/releases/download/$TAG/$f" \
        || die "the download of $f from release $TAG failed (see the curl message above)"
    done
  else
    gh release download "$TAG" --repo "$REPO" --dir "$T/dl" \
      --pattern "$BIN_TGZ" --pattern SHA256SUMS >/dev/null \
      || die "the download of $TAG failed (see the gh message above)"
  fi
  for f in "$BIN_TGZ" SHA256SUMS; do
    [ -f "$T/dl/$f" ] || die "release $TAG has no asset $f"
  done

  # 4. Verify the checksums.
  if command -v sha256sum >/dev/null 2>&1; then sha() { sha256sum "$1" | awk '{print $1}'; }
  elif command -v shasum >/dev/null 2>&1; then sha() { shasum -a 256 "$1" | awk '{print $1}'; }
  else die "neither sha256sum nor shasum is installed: cannot verify the download"; fi
  want=$(awk -v f="$BIN_TGZ" '$2 == f || $2 == "*" f {print $1; exit}' "$T/dl/SHA256SUMS")
  [ -n "$want" ] || die "SHA256SUMS has no line for $BIN_TGZ. Nothing was installed."
  got=$(sha "$T/dl/$BIN_TGZ")
  [ "$got" = "$want" ] || die "checksum mismatch for $BIN_TGZ (expected $want, got $got). Nothing was installed."
  echo "ok:   sha256 $BIN_TGZ"

  # 5. Unpack the binary and check it before anything is installed.
  mkdir -p "$T/bin"
  tar -xzf "$T/dl/$BIN_TGZ" -C "$T/bin" || die "cannot unpack $BIN_TGZ"
  NEWBIN="$T/bin/crewboard-$VER-$TARGET/crewboard"
  [ -f "$NEWBIN" ] || die "$BIN_TGZ has no crewboard-$VER-$TARGET/crewboard"
  chmod +x "$NEWBIN"
  got=$("$NEWBIN" --version 2>&1) || die "the downloaded binary does not run on this host: $got"
  [ "$got" = "crewboard $VER" ] || die "the downloaded binary prints '$got', expected 'crewboard $VER'"
  echo "ok:   $got runs on this host"

  # 6. The skill install checks its options (for example the runtime names) in a dry run first.
  # Exit 2 is a usage error: stop before the binary is installed.
  echo "--- skill install (plan)"
  "$NEWBIN" skill install --dry-run ${RUNTIMES[@]+"${RUNTIMES[@]}"}
  rc=$?
  [ "$rc" -ne 2 ] || die "crewboard skill install refused the options (see above). Nothing was installed."
  echo "---"

  DEST="$PREFIX/bin/crewboard"
  if [ "$DRY" -eq 1 ]; then
    echo "plan: install $NEWBIN as $DEST"
    echo "plan: run crewboard skill install${RUNTIMES[*]:+ ${RUNTIMES[*]}}"
    echo "dry run only: nothing changed. Run without --dry-run to install."
    exit 0
  fi

  # 7. Install the binary: copy next to the target, then rename (a running crewboard keeps its file).
  mkdir -p "$PREFIX/bin" || die "cannot make $PREFIX/bin"
  if ! { cp "$NEWBIN" "$DEST.new.$$" && chmod 755 "$DEST.new.$$" && mv -f "$DEST.new.$$" "$DEST"; }; then
    rm -f "$DEST.new.$$"; die "cannot install $DEST"
  fi
  echo "do:   installed $DEST"

  # 8. Install the skill with the installed binary.
  echo "--- skill install"
  "$DEST" skill install ${RUNTIMES[@]+"${RUNTIMES[@]}"}
  SKILL_RC=$?
  echo "---"

  # 9. Show the result and the next step.
  echo "$("$DEST" --version) installed at $DEST"
  case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "note: $PREFIX/bin is not on your PATH. Add it, for example: export PATH=\"$PREFIX/bin:\$PATH\"" ;;
  esac
  [ "$SKILL_RC" -eq 0 ] || echo "note: crewboard skill install finished with problems (see above)." >&2
  echo "upgrade: crewboard update"
  # The same next-step lines as the overview of `crewboard --help`, so that both say one thing.
  echo "New crew (owner): \`crewboard agent token\`, \`crewboard init\`,"
  echo "then for each agent \`crewboard agent add\` on its host and \`crewboard approve\`."
  echo "First try: https://github.com/arconsis/crewboard/blob/main/docs/quick-try.md"
  echo "New agent: \`crewboard agent add\`, then \`crewboard doctor --setup\`."
  exit "$SKILL_RC"
}

# The fallback for a repo that curl cannot read (a private fork or mirror): gh, its login, read access, and the tag. Sets TAG.
gh_tag() {
  command -v gh >/dev/null 2>&1 || die "cannot read release ${VERSION} of $REPO with curl (the repository may be private, or the release may not exist). Install gh (https://github.com/cli/cli#installation), run: gh auth login, and run this script again. Releases: $WEB/$REPO/releases"
  if ! gh auth status >/dev/null 2>&1; then
    {
      echo "error: cannot read release $VERSION of $REPO with curl, and gh is not logged in."
      echo "  If the repository is private (a fork or mirror), log in as yourself:  gh auth login --hostname github.com"
      echo "  Your GitHub account needs read access to $REPO. If it has none, ask a maintainer for read access."
      if [ -n "${GH_TOKEN:-}${GITHUB_TOKEN:-}" ]; then
        echo "  GH_TOKEN or GITHUB_TOKEN is set: gh uses that token instead of your login. Unset it and try again."
      fi
    } >&2
    exit 1
  fi
  if ! gh repo view "$REPO" --json name >/dev/null 2>&1; then
    {
      echo "error: your gh login cannot read $REPO (the repo may be private)."
      echo "  Ask a maintainer for read access for your GitHub account. Check the login with: gh auth status"
      echo "  Log in with another account:  gh auth login --hostname github.com"
      if [ -n "${GH_TOKEN:-}${GITHUB_TOKEN:-}" ]; then
        echo "  GH_TOKEN or GITHUB_TOKEN is set: gh uses that token instead of your login. Unset it and try again."
      fi
    } >&2
    exit 1
  fi
  if [ "$VERSION" = latest ]; then
    TAG=$(gh release view --repo "$REPO" --json tagName --jq .tagName 2>/dev/null) || TAG=""
    [ -n "$TAG" ] || die "$REPO has no release yet. Build from source: docs/guides/install.md"
  else
    TAG=$(gh release view "$VERSION" --repo "$REPO" --json tagName --jq .tagName 2>/dev/null) || TAG=""
    [ -n "$TAG" ] || die "$REPO has no release $VERSION. See the releases with: gh release list --repo $REPO"
  fi
}

main "$@"
