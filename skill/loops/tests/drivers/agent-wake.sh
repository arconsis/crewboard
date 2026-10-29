#!/bin/bash
# agent-wake.sh - test driver: maps the old option names of a wake script to `crewboard wake`. Use the binary:
#
#   crewboard wake install --agent-dir <dir> [--scheduler auto|launchd|systemd|cron|hermes] \
#       [--every 5m] [--tick] [--runtime ... --work-dir ... --env-file ...] [--dry-run]
#   crewboard wake uninstall --agent-dir <dir>
#   crewboard wake status --agent-dir <dir>
#
# `--install SCHED` and `--uninstall SCHED` call `crewboard wake install|uninstall` with the same
# options (`--role coordinator` is `--tick`) and print one deprecation line to stderr. Without
# them the script runs the wake as before, because the wrappers of an earlier `--install` call it
# (`exec /bin/bash agent-wake.sh ...`): it checks the options and runs `crewboard wake` through
# poll-and-wake.sh. Install again with `crewboard wake install` and the wrapper calls
# `crewboard wake` directly. Exit 4 on a config problem; other codes come from `crewboard`.
#
# Options (all of them go on unchanged to `crewboard wake [install|uninstall]`):
#   --role dev|coordinator   coordinator is `--tick`: no model, `crewboard tick` on every run
#   --runtime claude|codex|hermes --agent A --initiative I --env-file F --config-dir D --work-dir W
#   --shim-dir D --model ID --allow TOOL --runtime-flags S --runtime-bin P --prompt-extra T
#   --host-lock DIR --max-wake DUR --events-every S --no-events-hook --print-cmd
#   --install SCHED [--every DUR] [--replace] | --uninstall SCHED
#   --state-dir --lock-dir --log --work-backoff --heartbeat-every --retry-delays --stale-after
#   --empty-grace --crewboard
# bash 3.2 compatible.

HERE=$(cd "$(dirname "$0")" && pwd)
# a scheduler starts the driver with a minimal PATH: add the install dirs of crewboard at the END
for _p in "$HOME/.cargo/bin" "$HOME/.local/bin"; do
  case ":$PATH:" in *":$_p:"*) ;; *) [ -d "$_p" ] && PATH="$PATH:$_p" ;; esac
done
export PATH
abs() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$(pwd)" "$1" ;; esac; }
die() { echo "agent-wake: $1" >&2; exit 4; }

ROLE=dev; RUNTIME=""; AGENT=""; INITIATIVE=""; WORK_DIR=""; SHIM_DIR=""
FLAGS=""; FLAGS_SET=0; EXTRA=""; PRINT=0; RT_BIN=""; CB_BIN=""; ALLOW=""; MODEL=""
INSTALL=""; UNINSTALL=""; EVERY=5m; HOOK=1; HOOK_EVERY=300
ALLOWS=()
pass=()   # the options that go to `crewboard wake` first (the run)
fwd=()    # every option as it was given, for `crewboard wake install|uninstall`
while [ $# -gt 0 ]; do
  case "$1" in
    --role) ROLE="$2"; shift 2 ;;
    --runtime) RUNTIME="$2"; fwd+=("$1" "$2"); shift 2 ;;
    --agent) AGENT="$2"; pass+=("$1" "$2"); fwd+=("$1" "$2"); shift 2 ;;
    --initiative) INITIATIVE="$2"; pass+=("$1" "$2"); fwd+=("$1" "$2"); shift 2 ;;
    --env-file|--config-dir) pass+=("$1" "$2"); fwd+=("$1" "$2"); shift 2 ;;
    --work-dir) WORK_DIR="$2"; fwd+=("$1" "$2"); shift 2 ;;
    --shim-dir) SHIM_DIR="$2"; fwd+=("$1" "$2"); shift 2 ;;
    --runtime-flags) FLAGS="$2"; FLAGS_SET=1; fwd+=("$1" "$2"); shift 2 ;;
    --allow) ALLOW="$ALLOW,$2"; ALLOWS+=("$2"); fwd+=("$1" "$2"); shift 2 ;;
    --model) MODEL="$2"; fwd+=("$1" "$2"); shift 2 ;;
    --runtime-bin) RT_BIN="$2"; fwd+=("$1" "$2"); shift 2 ;;
    --prompt-extra) EXTRA="$2"; fwd+=("$1" "$2"); shift 2 ;;
    --host-lock) pass+=("$1" "$(abs "$2")"); fwd+=("$1" "$2"); shift 2 ;;
    --no-events-hook) HOOK=0; fwd+=("$1"); shift ;;
    --replace) fwd+=("$1"); shift ;;
    --events-every) HOOK_EVERY="$2"; fwd+=("$1" "$2"); shift 2 ;;
    --print-cmd) PRINT=1; shift ;;
    --install) INSTALL="$2"; shift 2 ;;
    --uninstall) UNINSTALL="$2"; shift 2 ;;
    --every) EVERY="$2"; shift 2 ;;
    --crewboard) CB_BIN="$2"; fwd+=("$1" "$2"); shift 2 ;;
    --state-dir|--lock-dir|--log|--work-backoff|--heartbeat-every|--stale-after|--empty-grace|--max-wake|--retry-delays)
      pass+=("$1" "$2"); fwd+=("$1" "$2"); shift 2 ;;
    -h|--help) sed -n '2,/^[^#]/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done
[ -n "$CB_BIN" ] && pass+=(--crewboard "$CB_BIN")

case "$ROLE" in
  dev|coordinator) ;;
  *) die "--role must be dev or coordinator" ;;
esac
# the install options are checked by `crewboard wake install` itself
if [ -z "$INSTALL" ] && [ -z "$UNINSTALL" ]; then
  [ -n "$INITIATIVE" ] || die "--initiative is required"
fi
if [ "$ROLE" = dev ] && [ -z "$INSTALL" ] && [ -z "$UNINSTALL" ]; then
  [ -n "$AGENT" ] || die "--agent is required for a dev agent"
  [ -n "$WORK_DIR" ] && [ -d "$WORK_DIR" ] || die "--work-dir must be an existing directory: $WORK_DIR"
  case "$RUNTIME" in claude|codex|hermes) ;; *) die "--runtime must be claude, codex or hermes" ;; esac
  case "$HOOK_EVERY" in ''|*[!0-9]*) die "--events-every needs whole seconds, got '$HOOK_EVERY'" ;; esac
  [ -z "$ALLOW" ] || [ "$RUNTIME" = claude ] || die "--allow is for --runtime claude (Codex and Hermes have no tool list)"
fi

# --- install and uninstall: `crewboard wake install|uninstall` ------------------------------------
if [ -n "$INSTALL" ] || [ -n "$UNINSTALL" ]; then
  WAKE_BIN=${CB_BIN:-${CREWBOARD_BIN:-crewboard}}
  command -v "$WAKE_BIN" >/dev/null 2>&1 \
    || die "crewboard not found on PATH ($PATH); install it, set CREWBOARD_BIN or pass --crewboard <absolute path>"
  "$WAKE_BIN" wake install --help >/dev/null 2>&1 \
    || die "crewboard too old: needs 'wake install' ($WAKE_BIN); update crewboard"
  [ "$ROLE" != coordinator ] || fwd=(--tick "${fwd[@]}")
  if [ -n "$INSTALL" ]; then
    exec "$WAKE_BIN" wake install --scheduler "$INSTALL" --every "$EVERY" "${fwd[@]}"
  fi
  # `wake uninstall` knows the options of the unit only
  un=()
  i=0
  while [ "$i" -lt "${#fwd[@]}" ]; do
    case "${fwd[$i]}" in
      --initiative|--config-dir|--env-file) un+=("${fwd[$i]}" "${fwd[$((i + 1))]}"); i=$((i + 2)) ;;
      --tick) i=$((i + 1)) ;;
      *) i=$((i + 2)) ;;
    esac
  done
  exec "$WAKE_BIN" wake uninstall --scheduler "$UNINSTALL" "${un[@]}"
fi

# --- the wake: `crewboard wake` builds the command and the prompt, polls and wakes ---------------
# The headless coordinator runs `crewboard tick` and starts no model. A dev agent gets its wake
# command from the runtime options (the table of the runtimes is in the binary).
[ "$PRINT" -eq 1 ] && pass+=(--print-cmd)
if [ "$ROLE" = coordinator ]; then
  exec bash "$HERE/poll-and-wake.sh" "${pass[@]}" --tick
fi
args=(--runtime "$RUNTIME" --work-dir "$WORK_DIR")
[ -n "$SHIM_DIR" ] && args+=(--shim-dir "$SHIM_DIR")
[ -n "$MODEL" ] && args+=(--model "$MODEL")
[ "$FLAGS_SET" -eq 1 ] && args+=(--runtime-flags "$FLAGS")
[ -n "$RT_BIN" ] && args+=(--runtime-bin "$RT_BIN")
[ -n "$EXTRA" ] && args+=(--prompt-extra "$EXTRA")
for a in "${ALLOWS[@]}"; do args+=(--allow "$a"); done
[ "$HOOK" -eq 1 ] || args+=(--no-events-hook)
[ "$HOOK" -eq 1 ] && [ "$RUNTIME" = claude ] && args+=(--events-every "$HOOK_EVERY")
exec bash "$HERE/poll-and-wake.sh" "${pass[@]}" "${args[@]}"
