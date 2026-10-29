#!/bin/bash
# scripts/shellcheck.sh - lint the shell scripts of crewboard (warning level and above).
# Usage: scripts/shellcheck.sh
# Checks install.sh and scripts/*.sh and scripts/maintainer/*.sh (not the tests).
# Exit 0: no findings (prints "shellcheck: ok"), or shellcheck is not installed (skipped).
# Exit 1: findings.
ROOT=$(cd "$(dirname "$0")/.." && pwd)
if ! command -v shellcheck >/dev/null 2>&1; then
  echo "shellcheck: skipped, shellcheck is not installed (brew install shellcheck, or apt install shellcheck)"
  exit 0
fi
cd "$ROOT" || exit 1
if shellcheck -S warning install.sh scripts/*.sh scripts/maintainer/*.sh; then
  echo "shellcheck: ok"
else
  exit 1
fi
