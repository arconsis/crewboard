#!/bin/sh
# Test driver: runs `crewboard lock` (the host lock) for test-locks.sh and test-loops.sh.
for d in "$HOME/.local/bin" "$HOME/.cargo/bin"; do
  case ":$PATH:" in *":$d:"*) ;; *) [ -d "$d" ] && PATH="$PATH:$d" ;; esac
done
export PATH
exec "${CREWBOARD_BIN:-crewboard}" lock "$@"
