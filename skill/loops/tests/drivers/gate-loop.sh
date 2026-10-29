#!/bin/sh
# Test driver: runs `crewboard repeat` for test-locks.sh. It maps `--cmd 'X'` to `-- bash -c 'X'`.
for d in "$HOME/.local/bin" "$HOME/.cargo/bin"; do
  case ":$PATH:" in *":$d:"*) ;; *) [ -d "$d" ] && PATH="$PATH:$d" ;; esac
done
export PATH
c=""; n=$#; past=0
while [ "$n" -gt 0 ]; do
  a=$1; shift; n=$((n - 1))
  if [ "$past" = 1 ]; then set -- "$@" "$a"; continue; fi
  case $a in
    --) past=1; set -- "$@" "$a" ;;
    --cmd) if [ "$n" -gt 0 ]; then c=$1; shift; n=$((n - 1)); else set -- "$@" "$a"; fi ;;
    --runs) set -- "$@" --times ;;
    --runs=*) set -- "$@" "--times=${a#--runs=}" ;;
    --repo) set -- "$@" --work-dir ;;
    --repo=*) set -- "$@" "--work-dir=${a#--repo=}" ;;
    --owner) set -- "$@" --holder ;;
    --owner=*) set -- "$@" "--holder=${a#--owner=}" ;;
    *) set -- "$@" "$a" ;;
  esac
done
[ -n "$c" ] && set -- "$@" -- bash -c "$c"
exec "${CREWBOARD_BIN:-crewboard}" repeat "$@"
