#!/bin/bash
# test-install-release.sh - proof for install.sh (the root script) and for its old name
# scripts/install-release.sh. No network: a fake `curl` and a fake `gh` serve the release assets
# from a temporary directory, and a fake `uname` sets the OS and CPU.
# Cases (each refusal also proves that nothing was installed):
#   1. green path: the binary lands in <prefix>/bin and the installed binary runs
#      `skill install` (with the --runtime names); a second run is idempotent; --version 0.2.0
#      and the macOS arm64 target pick the right assets
#   2. a bad checksum of the binary tarball is refused
#   3. an unknown CPU is refused with a clear message, before any gh call
#   4. no gh login, and a login without read access, give the access message
#   5. --dry-run changes nothing (HOME, prefix, the temporary directory)
#   6. an unknown runtime and an unknown release are refused before the binary is installed
#   7. a failing `skill install` leaves the binary installed and the script exits 1
#   8. curl (the repo is public): no gh call, latest and --version, a bad checksum, a missing
#      release, no --runtime means no --runtime argument, the script works from a pipe with
#      `bash -s --`, and the old scripts/install-release.sh (in the repo and alone) still works
# Cases 1 to 7 run the gh fallback (the fake curl answers 404, like a private fork or mirror).
# Usage: bash test-install-release.sh   Exit 0 only if every case passes. bash 3.2 compatible.
# INSTALL_RELEASE=<path> runs the cases against another copy of the script (for red proofs).

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
SCRIPT="${INSTALL_RELEASE:-$ROOT/install.sh}"
T=$(mktemp -d "${TMPDIR:-/tmp}/crewboard-release-test.XXXXXX") || exit 1
trap 'rm -rf "$T"' EXIT
FAIL=0
check() { # name, exit code of the test (0 = pass)
  if [ "$2" -eq 0 ]; then printf 'PASS  %s\n' "$1"; else printf 'FAIL  %s\n' "$1"; FAIL=1; fi
}
ok() { if "$@"; then echo 0; else echo 1; fi; }
has() { case "$OUT" in *"$1"*) return 0 ;; esac; return 1; }
tree() { (cd "$1" 2>/dev/null && find . | LC_ALL=C sort); }
if command -v sha256sum >/dev/null 2>&1; then sha() { sha256sum "$1" | awk '{print $1}'; }
else sha() { shasum -a 256 "$1" | awk '{print $1}'; }; fi

VER=0.2.0
ASSETS="$T/assets"

# The fake release v0.2.0: one binary tarball per target and SHA256SUMS. The fake binary answers
# --version and `skill install`: it records "<path of the binary>|<arguments>" in
# $HOME/.skill-install.log, refuses an unknown runtime with exit 2 like the real one, plans only
# with --dry-run, else writes the shared copy and its runtime links in $HOME, and exits with
# $FAKE_SKILL_RC (default 0).
mkrelease() {
  rm -rf "$ASSETS"; mkdir -p "$ASSETS/v$VER" "$T/build"
  for tgt in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
    d="$T/build/crewboard-$VER-$tgt"; rm -rf "$d"; mkdir -p "$d"
    cat >"$d/crewboard" <<FAKE
#!/bin/sh
[ "\$1" = --version ] && { echo "crewboard $VER"; exit 0; }
[ "\$1 \$2" = "skill install" ] || { echo "fake crewboard: unexpected call: \$*" >&2; exit 3; }
shift 2; dry=0; rts=""
a=""; for x in "\$@"; do a="\$a|\$x"; done
case "\$a" in *"|--dry-run"*) dry=1 ;; esac
while [ \$# -gt 0 ]; do
  case "\$1" in
    --dry-run) dry=1 ;;
    --runtime) shift; case "\$1" in claude|codex|copilot|hermes) rts="\$rts \$1" ;; *) echo "unknown runtime: \$1 (use claude, codex, copilot or hermes)" >&2; exit 2 ;; esac ;;
  esac
  shift
done
if [ "\$dry" = 1 ]; then echo "plan: ln -s shared link"; exit 0; fi
echo "\$0|skill|install\$a" >>"\$HOME/.skill-install.log"
mkdir -p "\$HOME/.config/agent-skills/crewboard"; : >"\$HOME/.config/agent-skills/crewboard/SKILL.md"
for r in \$rts; do mkdir -p "\$HOME/.\$r/skills"; ln -sfn "\$HOME/.config/agent-skills/crewboard" "\$HOME/.\$r/skills/crewboard"; done
echo "done. Shared copy: \$HOME/.config/agent-skills/crewboard"
exit \${FAKE_SKILL_RC:-0}
FAKE
    chmod +x "$d/crewboard"
    printf 'target %s\n' "$tgt" >"$d/README.md"
    tar -C "$T/build" -czf "$ASSETS/v$VER/crewboard-$VER-$tgt.tar.gz" "crewboard-$VER-$tgt"
  done
  (cd "$ASSETS/v$VER" && for f in crewboard-*.tar.gz; do printf '%s  %s\n' "$(sha "$f")" "$f"; done >SHA256SUMS)
}

# Fake gh: the subcommands that install-release.sh uses. Every call is logged.
# FAKE_GH_AUTH=1: not logged in (every call fails). FAKE_GH_ACCESS=1: no read access to the repo.
mkdir -p "$T/fakebin"
cat >"$T/fakebin/gh" <<'EOF'
#!/bin/bash
echo "gh $*" >>"$FAKE_GH_LOG"
[ "${FAKE_GH_AUTH:-0}" = 0 ] || { echo "You are not logged into any GitHub hosts." >&2; exit 1; }
case "$1 $2" in
  "auth status") exit 0 ;;
  "repo view") [ "${FAKE_GH_ACCESS:-0}" = 0 ] || { echo "GraphQL: Could not resolve to a Repository" >&2; exit 1; }; echo '{"name":"crewboard"}'; exit 0 ;;
  "release view")
    [ "${FAKE_GH_ACCESS:-0}" = 0 ] || { echo "release not found" >&2; exit 1; }
    shift 2; tag=""
    case "${1:-}" in -*|"") tag=$(/bin/ls "$FAKE_GH_DIR" | tail -n 1) ;; *) tag="$1" ;; esac
    [ -n "$tag" ] && [ -d "$FAKE_GH_DIR/$tag" ] || { echo "release not found" >&2; exit 1; }
    echo "$tag"; exit 0 ;;
  "release download")
    shift 2; tag="$1"; shift; dir=.; pats=()
    while [ $# -gt 0 ]; do
      case "$1" in
        --dir|-D) dir="$2"; shift ;;
        --pattern|-p) pats+=("$2"); shift ;;
        --repo|-R) shift ;;
      esac
      shift
    done
    [ -d "$FAKE_GH_DIR/$tag" ] || { echo "release not found" >&2; exit 1; }
    n=0
    for p in "${pats[@]}"; do
      if [ -f "$FAKE_GH_DIR/$tag/$p" ]; then cp "$FAKE_GH_DIR/$tag/$p" "$dir/"; n=$((n + 1)); fi
    done
    [ "$n" -gt 0 ] || { echo "no assets match the file pattern" >&2; exit 1; }
    exit 0 ;;
esac
echo "fake gh: unexpected call: $*" >&2; exit 3
EOF
# Fake curl: the URLs that install.sh uses. FAKE_CURL_MODE=private (a private fork or mirror): every release URL gives 404.
# Every call is logged in $FAKE_CURL_LOG.
cat >"$T/fakebin/curl" <<'EOF'
#!/bin/bash
echo "curl $*" >>"$FAKE_CURL_LOG"
out=""; fmt=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift ;;
    -w) fmt="$2"; shift ;;
    -*) ;;
    *) url="$1" ;;
  esac
  shift
done
notfound() { echo "curl: (22) The requested URL returned error: 404" >&2; exit 22; }
base="https://github.com/arconsis/crewboard"
case "$url" in
  https://raw.githubusercontent.com/*/install.sh) cp "$FAKE_INSTALL_SH" "$out"; exit 0 ;;
esac
[ "${FAKE_CURL_MODE:-private}" = public ] || notfound
case "$url" in
  */releases/latest) tag=$(/bin/ls "$FAKE_GH_DIR" | tail -n 1) ;;
  */releases/tag/*) tag=${url##*/releases/tag/} ;;
  */releases/download/*/*)
    rest=${url##*/releases/download/}; tag=${rest%%/*}; f=${rest#*/}
    [ -f "$FAKE_GH_DIR/$tag/$f" ] || notfound
    cp "$FAKE_GH_DIR/$tag/$f" "$out"; exit 0 ;;
  *) echo "fake curl: unexpected url: $url" >&2; exit 3 ;;
esac
[ -n "$tag" ] && [ -d "$FAKE_GH_DIR/$tag" ] || notfound
[ -z "$fmt" ] || printf '%s' "$base/releases/tag/$tag"
exit 0
EOF
cat >"$T/fakebin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo "$FAKE_UNAME_S" ;; -m) echo "$FAKE_UNAME_M" ;; *) echo "$FAKE_UNAME_S" ;; esac
EOF
chmod +x "$T/fakebin/gh" "$T/fakebin/uname" "$T/fakebin/curl"

# run <case> [args...]: a fresh HOME, prefix and TMPDIR per case. Sets OUT, RC, H, P, TD.
run() {
  c="$1"; shift
  H="$T/$c/home"; P="$T/$c/prefix"; TD="$T/$c/tmp"; rm -rf "${T:?}/$c"; mkdir -p "$H" "$P" "$TD"
  : >"$T/$c/gh.log"; : >"$T/$c/curl.log"
  OUT=$(env HOME="$H" TMPDIR="$TD" PATH="$T/fakebin:$PATH" FAKE_GH_DIR="$ASSETS" FAKE_GH_LOG="$T/$c/gh.log" \
    FAKE_CURL_LOG="$T/$c/curl.log" FAKE_CURL_MODE="${CURLMODE:-private}" FAKE_INSTALL_SH="$ROOT/install.sh" \
    FAKE_UNAME_S="${S:-Linux}" FAKE_UNAME_M="${M:-x86_64}" FAKE_GH_AUTH="${AUTH:-0}" FAKE_GH_ACCESS="${ACCESS:-0}" FAKE_SKILL_RC="${SKILL_RC:-0}" \
    GH_TOKEN="" GITHUB_TOKEN="" /bin/bash "$SCRIPT" --prefix "$P" "$@" 2>&1)
  RC=$?
}
rerun() { # the same HOME and prefix as the last run
  OUT=$(env HOME="$H" TMPDIR="$TD" PATH="$T/fakebin:$PATH" FAKE_GH_DIR="$ASSETS" FAKE_GH_LOG="$T/$c/gh.log" \
    FAKE_CURL_LOG="$T/$c/curl.log" FAKE_CURL_MODE="${CURLMODE:-private}" FAKE_INSTALL_SH="$ROOT/install.sh" \
    FAKE_UNAME_S="${S:-Linux}" FAKE_UNAME_M="${M:-x86_64}" GH_TOKEN="" GITHUB_TOKEN="" \
    /bin/bash "$SCRIPT" --prefix "$P" "$@" 2>&1)
  RC=$?
}
# shellcheck disable=SC2329 # called through ok()
nothing_installed() { [ ! -e "$P/bin" ] && [ ! -e "$H/.config" ] && [ ! -e "$H/.claude" ]; }
CANON=".config/agent-skills/crewboard"

mkrelease

# 1. green path
run green --runtime claude
check "green: exits 0" "$RC"
check "green: binary installed and executable" "$(ok test -x "$P/bin/crewboard")"
check "green: installed binary prints the version" "$(ok test "$("$P/bin/crewboard" --version)" = "crewboard $VER")"
check "green: the Linux x86_64 tarball was downloaded" "$(ok grep -q "crewboard-$VER-x86_64-unknown-linux-gnu.tar.gz" "$T/green/gh.log")"
check "green: the installed binary ran skill install with the runtime" "$(ok test "$(cat "$H/.skill-install.log")" = "$P/bin/crewboard|skill|install|--runtime|claude")"
check "green: skill installed in HOME" "$(ok test -f "$H/$CANON/SKILL.md")"
check "green: no skill tarball was asked for" "$(ok test -z "$(grep -i 'skill-' "$T/green/gh.log")")"
check "green: claude link to the shared copy" "$(ok test "$(readlink "$H/.claude/skills/crewboard")" = "$H/$CANON")"
check "green: the checksum is verified" "$(has "ok:   sha256 crewboard-$VER-x86_64-unknown-linux-gnu.tar.gz"; echo $?)"
check "green: prints the version and the next step" "$(has "crewboard $VER installed at $P/bin/crewboard" && has "New crew (owner):" && has "First try:"; echo $?)"
check "green: the temporary directory is removed" "$(ok test -z "$(/bin/ls -A "$TD")")"
before_bin=$(tree "$P"); before_home=$(tree "$H")
rerun --runtime claude
check "green: a second run exits 0 (idempotent)" "$RC"
check "green: a second run leaves the same files" "$(ok test "$before_bin$before_home" = "$(tree "$P")$(tree "$H")")"
run version --version 0.2.0
check "green: --version 0.2.0 installs v0.2.0" "$(ok test "$RC" -eq 0 -a -x "$P/bin/crewboard")"
check "green: --version asks for the tag v0.2.0" "$(ok grep -q "gh release view v$VER" "$T/version/gh.log")"
S=Darwin M=arm64 run mac
check "green: macOS arm64 exits 0" "$RC"
check "green: macOS arm64 downloads the aarch64-apple-darwin tarball" "$(ok grep -q "crewboard-$VER-aarch64-apple-darwin.tar.gz" "$T/mac/gh.log")"
S=Linux M=aarch64 run linarm
check "green: Linux aarch64 downloads the aarch64-unknown-linux-gnu tarball" "$(ok test "$RC" -eq 0 -a -n "$(grep "crewboard-$VER-aarch64-unknown-linux-gnu.tar.gz" "$T/linarm/gh.log")")"

# 2. bad checksum
good=$(/bin/cat "$ASSETS/v$VER/SHA256SUMS")
awk -v f="crewboard-$VER-x86_64-unknown-linux-gnu.tar.gz" '$2 == f {$1 = "0000000000000000000000000000000000000000000000000000000000000000"} {print $1 "  " $2}' \
  <<<"$good" >"$ASSETS/v$VER/SHA256SUMS"
run badsum --runtime claude
check "bad checksum (binary): refused" "$(ok test "$RC" -ne 0)"
check "bad checksum (binary): says checksum mismatch" "$(has "checksum mismatch for crewboard-$VER-x86_64-unknown-linux-gnu.tar.gz"; echo $?)"
check "bad checksum (binary): nothing installed" "$(ok nothing_installed)"
printf '%s\n' "$good" >"$ASSETS/v$VER/SHA256SUMS"
mkrelease

# 3. unknown CPU
M=riscv64 run arch --runtime claude
check "unknown arch: refused" "$(ok test "$RC" -ne 0)"
check "unknown arch: names the OS and CPU and the source build" "$(has "no prebuilt binary for Linux/riscv64" && has "Build from source"; echo $?)"
check "unknown arch: no gh call" "$(ok test ! -s "$T/arch/gh.log")"
check "unknown arch: nothing installed" "$(ok nothing_installed)"

# 4. no gh login, no read access
AUTH=1 run nologin --runtime claude
check "no login: refused" "$(ok test "$RC" -ne 0)"
check "no login: says not logged in and the gh auth command" "$(has "gh is not logged in" && has "gh auth login --hostname github.com"; echo $?)"
check "no login: says ask a maintainer for read access" "$(has "ask a maintainer for read access"; echo $?)"
check "no login: nothing installed" "$(ok nothing_installed)"
ACCESS=1 run noaccess --runtime claude
check "no access: refused" "$(ok test "$RC" -ne 0)"
check "no access: says ask a maintainer for read access" "$(has "cannot read arconsis/crewboard" && has "Ask a maintainer for read access"; echo $?)"
check "no access: nothing installed" "$(ok nothing_installed)"

# 5. --dry-run changes nothing
run dry --runtime claude --dry-run
check "dry run: exits 0" "$RC"
check "dry run: says nothing changed" "$(has "dry run only: nothing changed" && has "plan: install"; echo $?)"
check "dry run: shows the plan of skill install" "$(has "plan: ln -s"; echo $?)"
check "dry run: HOME and prefix stay empty" "$(ok test -z "$(/bin/ls -A "$H")$(/bin/ls -A "$P")")"
check "dry run: the temporary directory is removed" "$(ok test -z "$(/bin/ls -A "$TD")")"
check "dry run: it downloaded and verified the release" "$(has "ok:   sha256 crewboard-$VER-x86_64-unknown-linux-gnu.tar.gz"; echo $?)"
check "dry run: skill install ran in plan mode only" "$(ok test ! -e "$H/.skill-install.log")"

# 6. unknown runtime, unknown release
run badrt --runtime vim
check "unknown runtime: refused" "$(ok test "$RC" -ne 0)"
check "unknown runtime: names the runtime" "$(has "unknown runtime: vim"; echo $?)"
check "unknown runtime: nothing installed" "$(ok nothing_installed)"
run norel --version v9.9.9
check "unknown release: refused" "$(ok test "$RC" -ne 0)"
check "unknown release: says it has no release v9.9.9" "$(has "has no release v9.9.9"; echo $?)"
check "unknown release: nothing installed" "$(ok nothing_installed)"

# 7. skill install fails: the binary stays, the script exits 1 and says so
SKILL_RC=1 run skillfail --runtime claude
check "skill install fails: exit 1" "$(ok test "$RC" -eq 1)"
check "skill install fails: the binary is installed" "$(ok test -x "$P/bin/crewboard")"
check "skill install fails: says so" "$(has "crewboard skill install finished with problems"; echo $?)"
# the green run above exits 0 with the same script: the exit code follows skill install

# 8. curl: the repo is public
CURLMODE=public
run pub --runtime claude
check "curl: exits 0" "$RC"
check "curl: no gh call" "$(ok test ! -s "$T/pub/gh.log")"
check "curl: the tag came from the latest page" "$(ok grep -q "releases/latest" "$T/pub/curl.log")"
check "curl: the tarball came from the release download URL" "$(ok grep -q "releases/download/v$VER/crewboard-$VER-x86_64-unknown-linux-gnu.tar.gz" "$T/pub/curl.log")"
check "curl: SHA256SUMS came from the release download URL" "$(ok grep -q "releases/download/v$VER/SHA256SUMS" "$T/pub/curl.log")"
check "curl: the checksum is verified" "$(has "ok:   sha256 crewboard-$VER-x86_64-unknown-linux-gnu.tar.gz"; echo $?)"
check "curl: binary installed and the skill linked" "$(ok test -x "$P/bin/crewboard" -a -L "$H/.claude/skills/crewboard")"
check "curl: prints the upgrade command" "$(has "upgrade: crewboard update"; echo $?)"
check "curl: the temporary directory is removed" "$(ok test -z "$(/bin/ls -A "$TD")")"
run pubver --version 0.2.0
check "curl: --version 0.2.0 asks for the page of the tag v0.2.0" "$(ok test "$RC" -eq 0 && grep -q "releases/tag/v$VER" "$T/pubver/curl.log" && test ! -s "$T/pubver/gh.log")"
run pubnort
check "no --runtime: skill install gets no --runtime argument" "$(ok test "$RC" -eq 0 -a "$(cat "$H/.skill-install.log")" = "$P/bin/crewboard|skill|install")"
run pubrts --runtime codex --runtime hermes
check "--runtime twice: both reach skill install" "$(ok test "$(cat "$H/.skill-install.log")" = "$P/bin/crewboard|skill|install|--runtime|codex|--runtime|hermes")"
run pubdry --dry-run
check "curl dry run: exits 0, changes nothing" "$(ok test "$RC" -eq 0 -a -z "$(/bin/ls -A "$H")$(/bin/ls -A "$P")")"
check "curl dry run: says nothing changed" "$(has "dry run only: nothing changed"; echo $?)"
AUTH=1 run pubmiss --version v9.9.9
check "curl, gh not logged in, unknown release: refused" "$(ok test "$RC" -ne 0)"
check "curl, unknown release: says curl could not read it and points to gh auth login" "$(has "cannot read release v9.9.9 of arconsis/crewboard with curl" && has "gh auth login"; echo $?)"
run pubmiss2 --version v9.9.9
check "curl fails, gh logged in: the gh fallback names the missing release" "$(has "has no release v9.9.9"; echo $?)"
check "curl, unknown release: nothing installed" "$(ok nothing_installed)"
awk -v f="crewboard-$VER-x86_64-unknown-linux-gnu.tar.gz" '$2 == f {$1 = "0000000000000000000000000000000000000000000000000000000000000000"} {print $1 "  " $2}' \
  <<<"$good" >"$ASSETS/v$VER/SHA256SUMS"
run pubbad
check "curl, bad checksum: refused, nothing installed" "$(ok test "$RC" -ne 0 && has "checksum mismatch" && nothing_installed)"
mkrelease

# 8b. from a pipe, as in the README: curl ... | bash -s -- --version v0.2.0
H="$T/pipe/home"; P="$T/pipe/prefix"; TD="$T/pipe/tmp"; rm -rf "${T:?}/pipe"; mkdir -p "$H" "$P" "$TD"; : >"$T/pipe/gh.log"; : >"$T/pipe/curl.log"
OUT=$(cat "$ROOT/install.sh" | env HOME="$H" TMPDIR="$TD" PATH="$T/fakebin:$PATH" FAKE_GH_DIR="$ASSETS" FAKE_GH_LOG="$T/pipe/gh.log" \
  FAKE_CURL_LOG="$T/pipe/curl.log" FAKE_CURL_MODE=public FAKE_UNAME_S=Linux FAKE_UNAME_M=x86_64 GH_TOKEN="" GITHUB_TOKEN="" \
  /bin/bash -s -- --version v0.2.0 --prefix "$P" 2>&1); RC=$?
check "pipe: bash -s -- passes the options" "$(ok test "$RC" -eq 0 -a -x "$P/bin/crewboard" && has "release: arconsis/crewboard v$VER")"
OUT=$(cat "$ROOT/install.sh" | /bin/bash -s -- --help 2>&1); RC=$?
check "pipe: --help prints the usage without a file name" "$(ok test "$RC" -eq 0 && has "Usage: install.sh")"

# 8c. the old name: in the repo it runs install.sh; alone (a copy in a bare directory) it fetches it
OLD="$ROOT/scripts/install-release.sh"
H="$T/old/home"; P="$T/old/prefix"; TD="$T/old/tmp"; rm -rf "${T:?}/old"; mkdir -p "$H" "$P" "$TD"; : >"$T/old/gh.log"; : >"$T/old/curl.log"
OUT=$(env HOME="$H" TMPDIR="$TD" PATH="$T/fakebin:$PATH" FAKE_GH_DIR="$ASSETS" FAKE_GH_LOG="$T/old/gh.log" \
  FAKE_CURL_LOG="$T/old/curl.log" FAKE_CURL_MODE=public FAKE_UNAME_S=Linux FAKE_UNAME_M=x86_64 GH_TOKEN="" GITHUB_TOKEN="" \
  /bin/bash "$OLD" --prefix "$P" --runtime claude 2>&1); RC=$?
check "old name in the repo: installs through install.sh" "$(ok test "$RC" -eq 0 -a -x "$P/bin/crewboard" -a -L "$H/.claude/skills/crewboard")"
mkdir -p "$T/bare"; cp "$OLD" "$T/bare/install-release.sh"
H="$T/old2/home"; P="$T/old2/prefix"; TD="$T/old2/tmp"; rm -rf "${T:?}/old2"; mkdir -p "$H" "$P" "$TD"; : >"$T/old2/gh.log"; : >"$T/old2/curl.log"
OUT=$(env HOME="$H" TMPDIR="$TD" PATH="$T/fakebin:$PATH" FAKE_GH_DIR="$ASSETS" FAKE_GH_LOG="$T/old2/gh.log" FAKE_INSTALL_SH="$ROOT/install.sh" \
  FAKE_CURL_LOG="$T/old2/curl.log" FAKE_CURL_MODE=public FAKE_UNAME_S=Linux FAKE_UNAME_M=x86_64 GH_TOKEN="" GITHUB_TOKEN="" \
  /bin/bash "$T/bare/install-release.sh" --prefix "$P" --runtime claude 2>&1); RC=$?
check "old name alone: gets install.sh with curl and installs" "$(ok test "$RC" -eq 0 -a -x "$P/bin/crewboard" && grep -q "raw.githubusercontent.com/arconsis/crewboard/main/install.sh" "$T/old2/curl.log")"
check "old name alone: the temporary directory is removed" "$(ok test -z "$(/bin/ls -A "$TD")")"

echo "---"
if [ "$FAIL" -eq 0 ]; then echo "all install-release tests passed"; else echo "install-release tests FAILED"; fi
exit "$FAIL"
