#!/bin/sh
# Test driver: runs `crewboard wake` for test-loops.sh and test-locks.sh.
for d in "$HOME/.cargo/bin" "$HOME/.local/bin"; do
  case ":$PATH:" in *":$d:"*) ;; *) [ -d "$d" ] && PATH="$PATH:$d" ;; esac
done
export PATH
# The binary: the `--crewboard BIN` of the arguments when it is an executable file (every --install
# wrapper stores an absolute one), else $CREWBOARD_BIN, else the PATH. The flag still goes on to
# `crewboard wake`.
bin=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--crewboard" ] && [ -f "$a" ] && [ -x "$a" ]; then bin=$a; fi
  prev=$a
done
[ -n "$bin" ] || bin=${CREWBOARD_BIN:-crewboard}
if ! command -v "$bin" >/dev/null 2>&1; then
  echo "poll-and-wake: crewboard not found on PATH ($PATH); install it or set CREWBOARD_BIN" >&2
  exit 127
fi
# A binary without `wake` ends with the usage exit 2, which looks like a refused poll.
if ! "$bin" wake --help >/dev/null 2>&1; then
  echo "poll-and-wake: crewboard too old: needs the wake command ($bin); update crewboard" >&2
  exit 127
fi
exec "$bin" wake "$@"
