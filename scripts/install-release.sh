#!/bin/bash
# install-release.sh - the old name of the installer. The installer is `install.sh` in the root of
# the repo (curl -fsSL https://raw.githubusercontent.com/arconsis/crewboard/main/install.sh | bash).
# This wrapper keeps the old command working: it runs that script with the same options.
# A copy of this file without the repo around it (for example one that `gh api` saved to /tmp)
# gets install.sh from the repo, with curl, else with gh. bash 3.2 compatible.

REPO="${CREWBOARD_RELEASE_REPO:-arconsis/crewboard}"
here=$(cd "$(dirname "$0")" && pwd)
if [ -f "$here/../install.sh" ]; then
  exec bash "$here/../install.sh" "$@"
fi
T=$(mktemp -d "${TMPDIR:-/tmp}/crewboard-installer.XXXXXX") || { echo "error: cannot make a temporary directory" >&2; exit 1; }
trap 'rm -rf "$T"' EXIT
if ! { command -v curl >/dev/null 2>&1 \
       && curl -fsSL -o "$T/install.sh" "${CREWBOARD_RAW_WEB:-https://raw.githubusercontent.com}/$REPO/main/install.sh" 2>/dev/null; } \
   && ! { command -v gh >/dev/null 2>&1 \
       && gh api -H 'Accept: application/vnd.github.raw' "repos/$REPO/contents/install.sh?ref=main" >"$T/install.sh" 2>/dev/null; }; then
  echo "error: cannot get install.sh of $REPO. Use: curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | CREWBOARD_RELEASE_REPO=$REPO bash" >&2
  echo "       For a private fork or mirror: gh api -H 'Accept: application/vnd.github.raw' repos/$REPO/contents/install.sh | CREWBOARD_RELEASE_REPO=$REPO bash" >&2
  exit 1
fi
bash "$T/install.sh" "$@"
