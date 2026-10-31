#!/bin/bash
# test-loops.sh - token-economy proof for `crewboard monitor` and the poll-and-wake driver.
# Uses a FAKE `crewboard` (stub on PATH) with scripted exit codes. No network, no tokens.
# Every case proves both directions: silence when nothing happens, one wake when something does.
# Covers `crewboard monitor`, the drivers poll-and-wake.sh and agent-wake.sh, and the commands
# `agent with-env`, `agent shim`, `agent env`, `agent exec` and `agent add` (cases E, G, C, XE and B),
# and the PATH that a scheduler gives the drivers (cases H1 to H5). The scheduler commands (systemctl, launchctl, crontab) are fakes
# too: no case installs a real unit.
# Usage: bash skill/loops/tests/test-loops.sh        Exit 0 only if every case passes. bash 3.2 compatible.

HERE=$(cd "$(dirname "$0")/.." && pwd)   # skill/loops/, the scripts under test
DRV="$HERE/tests/drivers"   # test drivers: thin wrappers that run the real binary
# A suite that cannot find a driver it needs must FAIL, not run zero cases.
for s in tests/drivers/poll-and-wake.sh tests/drivers/agent-wake.sh; do
  [ -f "$HERE/$s" ] || { echo "FAIL  script under test not found: $HERE/$s" >&2; exit 2; }
done
P="$DRV/poll-and-wake.sh"
T=$(mktemp -d "${TMPDIR:-/tmp}/crewboard-loops.XXXXXX") || exit 1
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin" "$T/cfg-none"
export T
# this wrapper makes the real `crewboard monitor` run the fake crewboard below as its `watch`
# (the hidden test seam --crewboard)
W="$T/bin/monitor-wrapper"
# the real binary, for the cases that run the host lock and the agent commands: found BEFORE the
# fake goes on the PATH
CREWBOARD_REAL=${CREWBOARD_REAL:-$(command -v crewboard)}
[ -n "$CREWBOARD_REAL" ] && [ -x "$CREWBOARD_REAL" ] || { echo "FAIL  no crewboard binary: set CREWBOARD_REAL or put a built crewboard on the PATH" >&2; exit 2; }
export CREWBOARD_REAL

# --- the fake crewboard: pops one plan line per call: <code>|<stdout>|<stderr>
# When the plan is empty it returns $FAKE_DEFAULT (default 1). It records every call.
# `mine` and `heartbeat` calls have their own record (calls.mine, calls.hb) and, for `mine`,
# their own plan (plan.mine, default exit 1), so the poll numbering stays the same.
cat >"$T/bin/crewboard" <<'STUB'
#!/bin/bash
D="$FAKE_DIR"
# The host lock (`lock`), the silent watch loop (`monitor`) and the env file of an agent
# (`agent exec|env|with-env|add`) are commands of the real binary. The command is the first word that is not a
# global option or its value. The config dir defaults to an empty one, so that the agent.env of
# the user running this suite is never read.
sub=""; skip=0
for a in "$@"; do
  if [ "$skip" = 1 ]; then skip=0; continue; fi
  case "$a" in
    --config-dir|--env-file|--heartbeat-interval) skip=1 ;;
    -*) ;;
    *) sub=$a; break ;;
  esac
done
case "$sub" in
  lock|monitor|agent|hook) exec env CREWBOARD_CONFIG_DIR="${CREWBOARD_CONFIG_DIR:-$T/cfg-none}" "$CREWBOARD_REAL" "$@" ;;
  wake)
    # the drivers poll-and-wake.sh and agent-wake.sh run `crewboard wake`: it asks THIS stub for the board
    # (the hidden --crewboard seam), unless the case names one itself.
    seam=1; for a in "$@"; do [ "$a" = --crewboard ] && seam=0; done
    [ "$seam" = 0 ] || set -- "$@" --crewboard "$0"
    exec env CREWBOARD_CONFIG_DIR="${CREWBOARD_CONFIG_DIR:-$T/cfg-none}" "$CREWBOARD_REAL" "$@" ;;
esac
for a in "$@"; do
  if [ "$a" = mine ]; then
    printf '%s\n' "$*" >>"$D/calls.mine"
    n=$(wc -l <"$D/calls.mine" | tr -d ' ')
    # a plan.mine line is <code> or <code>|<stdout>
    ml=$(sed -n "${n}p" "$D/plan.mine" 2>/dev/null | grep . || echo 1)
    case "$ml" in *'|'*) printf '%b\n' "${ml#*|}"; ml=${ml%%|*} ;; esac
    exit "$ml"
  fi
  if [ "$a" = heartbeat ]; then printf '%s\n' "$*" >>"$D/calls.hb"; exit 0; fi
  if [ "$a" = acted ]; then
    # the event receipts, with their own record (calls.acted) and no plan line.
    # --list prints acted.list. The check takes the receipts of acted.check (the events that
    # the model did not act on); one that stands in the events file behind a REDELIVERED line is
    # "twice", else "again" and the stub writes the REDELIVERED line to --redeliver-to.
    # acted.rc is the exit code of both calls (default 0).
    printf '%s\n' "$*" >>"$D/calls.acted"
    rc=0; [ -f "$D/acted.rc" ] && rc=$(cat "$D/acted.rc")
    [ "$rc" -eq 0 ] || { echo "error: unrecognized subcommand 'acted'" >&2; exit "$rc"; }
    case " $* " in *" --list "*) [ -f "$D/acted.list" ] && cat "$D/acted.list"; exit 0 ;; esac
    ef=""; rd=""
    while [ $# -gt 0 ]; do
      case "$1" in --events-file) ef=$2 ;; --redeliver-to) rd=$2 ;; esac
      shift
    done
    [ -f "$D/acted.check" ] || exit 0
    while IFS= read -r r; do
      [ -n "$r" ] || continue
      if grep -qxF "REDELIVERED $r" "$ef" 2>/dev/null; then echo "$r twice"
      else echo "$r again"; printf 'REDELIVERED %s\n%s\n\n' "$r" "block of $r" >>"$rd"; fi
    done <"$D/acted.check"
    exit 0
  fi
done
printf '%s\n' "$*" >>"$D/calls"
printf '%s\n' "${GH_TOKEN:-unset}" >"$D/seen-token"
n=$(wc -l <"$D/calls" | tr -d ' ')
line=$(sed -n "${n}p" "$D/plan" 2>/dev/null)
if [ -z "$line" ]; then exit "${FAKE_DEFAULT:-1}"; fi
code=${line%%|*}; rest=${line#*|}; out=${rest%%|*}; err=${rest#*|}
[ -n "$out" ] && printf '%b\n' "$out"
[ -n "$err" ] && printf '%b\n' "$err" >&2
exit "$code"
STUB
chmod +x "$T/bin/crewboard"
printf '%s\n' '#!/bin/sh' "exec env CREWBOARD_CONFIG_DIR=\"\${CREWBOARD_CONFIG_DIR:-$T/cfg-none}\" '$CREWBOARD_REAL' monitor --crewboard '$T/bin/crewboard' \"\$@\"" >"$W"
chmod +x "$W"
# a fake gh for the PR lookup of the budget stop: `pr view` prints gh.view of the
# case (exit 1 without it), `pr list` prints gh.list. It records every call in gh.calls.
cat >"$T/bin/gh" <<'STUB'
#!/bin/bash
printf '%s\n' "$*" >>"$FAKE_DIR/gh.calls"
case "$1 $2" in
  "pr view") [ -f "$FAKE_DIR/gh.view" ] || exit 1; cat "$FAKE_DIR/gh.view" ;;
  "pr list") [ -f "$FAKE_DIR/gh.list" ] && cat "$FAKE_DIR/gh.list" ;;
esac
exit 0
STUB
chmod +x "$T/bin/gh"
export PATH="$T/bin:$PATH"
# CB_IMPL=rust: a dry parity run. The poll-and-wake cases of the list PORTED run through the hidden
# `crewboard wake` of the real binary (the wrapper pw) instead of poll-and-wake.sh, and the calls
# to the board go to the fake above (--crewboard). The run stops after the hook cases (HW5);
# every case outside the list is skipped, not failed. Default: the shell script runs as before.
PW="$T/bin/pw"
cat >"$PW" <<EOF_PW
#!/bin/sh
export CREWBOARD_CONFIG_DIR="\${CREWBOARD_CONFIG_DIR:-$T/cfg-none}"
exec '$CREWBOARD_REAL' wake --crewboard '$T/bin/crewboard' "\$@"
EOF_PW
chmod +x "$PW"
PORTED=" P1 P2 P3 P3b P3c P3d P3e Q1 Q2 Q3 P4 P5 P5b P5c P5d P6 P7 P8 P9 P10 P10b P11 S1 S1b S2 R1 R2 R2b X1 X1b X2 EV2 EV3 AC1 AC2 AC3 AC5 AC6 RS1 RS1b RS1c RS2 RS3 RS4 HK1 HK1b HK2 HK2b HK3 HK4 HK4b HK5 HK6 A1 A1b A2 A3 A4 A4b A5 A5b A5c A6 A7 A8 A8b A8c A8d A9 A10 HW1 HW1b HW2 HW3 HW4 HW5 CO1 CO2 CO3 CO4 P12 P12b TB1 TB2 TB3 TB4 TB5 TB6 TB7 TB10 TB11 TB11b "
SKIPPED=0
if [ "${CB_IMPL:-shell}" = rust ]; then P="$PW"; fi
# No retry of a crewboard exit 3 and no time budget unless a case asks for one.
export CREWBOARD_RETRY_DELAYS=""
export CREWBOARD_MAX_WAKE=off
# The state of the scripts stays in the test dir, never in the state dir of this user.
export XDG_STATE_HOME="$T/state"

finish() { # the summary and the exit code
  echo
  [ "${CASES:-0}" -gt 0 ] || { echo "SUMMARY: FAILED (no case ran)"; exit 1; }
  echo "cases run: $CASES"
  [ "$SKIPPED" -eq 0 ] || echo "cases skipped (not ported to crewboard wake yet): $SKIPPED"
  if [ "$FAIL" -eq 0 ]; then echo "SUMMARY: all cases passed"; exit 0; else echo "SUMMARY: FAILED"; exit 1; fi
}

FAIL=0
RESULTS=""
case_result() { # name, ok(0/1), detail
  if [ "${CB_IMPL:-shell}" = rust ]; then
    case "$1" in
    esac
    case "$PORTED" in *" ${1%% *} "*) ;; *) SKIPPED=$((SKIPPED + 1)); return 0 ;; esac
  fi
  CASES=$((CASES + 1))
  if [ "$2" -eq 0 ]; then RESULTS="$RESULTS\nPASS  $1  ($3)"; else RESULTS="$RESULTS\nFAIL  $1  ($3)"; FAIL=1; fi
  if [ "$2" -eq 0 ]; then printf 'PASS  %-58s %s\n' "$1" "$3"; else printf 'FAIL  %-58s %s\n' "$1" "$3"; fi
}
new_case() { # fresh fake dir; args: plan lines
  export FAKE_DIR="$T/case.$1"; rm -rf "$FAKE_DIR"; mkdir -p "$FAKE_DIR"
  : >"$FAKE_DIR/calls"; : >"$FAKE_DIR/plan"
  shift
  for l in "$@"; do printf '%s\n' "$l" >>"$FAKE_DIR/plan"; done
}
ncalls() { wc -l <"$FAKE_DIR/calls" | tr -d ' '; }
nlines() { if [ -f "$1" ]; then wc -l <"$1" | tr -d ' '; else echo 0; fi; }
rep() { local i=0 s=""; while [ $i -lt "$1" ]; do s="$s$2\n"; i=$((i + 1)); done; printf '%b' "$s"; }


# ---------------- monitor ----------------
echo "== crewboard monitor"

# W1: 20 cycles of exit 1 -> 0 bytes on stdout, 20 calls, exit 0
new_case w1
rep 20 '1|no events cursor=abc polls=3 last-ok=now|' >"$FAKE_DIR/plan"
"$W" --pause 0 --max-cycles 20 --timeout 1m --config-dir "$T/cfg" >"$FAKE_DIR/stdout" 2>"$FAKE_DIR/stderr"; echo $? >"$FAKE_DIR/rc"
bytes=$(wc -c <"$FAKE_DIR/stdout" | tr -d ' ')
ok=1; [ "$bytes" -eq 0 ] && [ "$(ncalls)" -eq 20 ] && [ "$(cat "$FAKE_DIR/rc")" -eq 0 ] && ok=0
case_result "W1 20x exit 1 -> silent" $ok "stdout=${bytes} bytes, crewboard calls=$(ncalls), rc=$(cat "$FAKE_DIR/rc")"

# W2: one exit 0 among silent cycles -> exactly one output block, loop returns
new_case w2
{ rep 7 '1|no events cursor=abc|'; echo '0|EVENT 1: assign #12 to claude@host-a\nEVENT 2: owner message|'; } >"$FAKE_DIR/plan"
"$W" --pause 0 --timeout 1m --config-dir "$T/cfg" >"$FAKE_DIR/stdout" 2>/dev/null; rc=$?
exp=$(printf 'EVENT 1: assign #12 to claude@host-a\nEVENT 2: owner message')
got=$(cat "$FAKE_DIR/stdout")
ok=1; [ "$rc" -eq 0 ] && [ "$got" = "$exp" ] && [ "$(ncalls)" -eq 8 ] && ok=0
case_result "W2 7x exit 1 then exit 0 -> one block, exit 0" $ok "calls=$(ncalls), output lines=$(nlines "$FAKE_DIR/stdout"), rc=$rc"

# W2b: the exit-1 cursor line of crewboard never leaks into the output
leak=0; grep -q 'cursor=abc' "$FAKE_DIR/stdout" && leak=1
case_result "W2b cursor line of exit 1 does not leak" $leak "leak=$([ $leak -eq 0 ] && echo no || echo YES)"

# W3: exit 3 -> one error line, exit 3
new_case w3
{ rep 2 '1||'; echo '3||HTTP 502 from api.github.com'; } >"$FAKE_DIR/plan"
"$W" --pause 0 --timeout 1m >"$FAKE_DIR/stdout" 2>/dev/null; rc=$?
got=$(cat "$FAKE_DIR/stdout")
ok=1; [ "$rc" -eq 3 ] && [ "$(nlines "$FAKE_DIR/stdout")" -eq 1 ] && [ "$got" = "crewboard-watch: exit 3: HTTP 502 from api.github.com" ] && ok=0
case_result "W3 exit 3 -> one error line, rc 3" $ok "rc=$rc, line='$got'"

# W3b: exit 4 and exit 2 are passed through
new_case w3b; echo '4||key file missing' >"$FAKE_DIR/plan"
"$W" --pause 0 >"$FAKE_DIR/stdout" 2>/dev/null; rc4=$?
new_case w3c; echo '2||rule not-approved refused' >"$FAKE_DIR/plan"
"$W" --pause 0 >"$FAKE_DIR/stdout" 2>/dev/null; rc2=$?
ok=1; [ "$rc4" -eq 4 ] && [ "$rc2" -eq 2 ] && ok=0
case_result "W3b exit 4 and exit 2 pass through" $ok "rc=$rc4 and $rc2"

# W3d: exit >= 2 shows the FIRST stderr line that is not the status line, plus the status line
#. crewboard prints the reason first and the status line last; the old loop showed
# only the last line. The reason has a control character and is longer than 200 characters.
new_case w3d
printf '4||configuration error: no board file for initiative demo\033[31m %s\\nhint: run crewboard join first\\ncursor=unknown polls=0 last_ok=never\n' \
  "$(rep 300 'Z' | tr -d '\n')" >"$FAKE_DIR/plan"
"$W" --pause 0 --log "$FAKE_DIR/loop.log" >"$FAKE_DIR/stdout" 2>/dev/null; rc=$?
got=$(cat "$FAKE_DIR/stdout")
reason=${got#crewboard-watch: exit 4: }; reason=${reason% (cursor=*}
ok=1; [ "$rc" -eq 4 ] && [ "$(nlines "$FAKE_DIR/stdout")" -eq 1 ] \
  && printf '%s' "$got" | grep -q '^crewboard-watch: exit 4: configuration error: no board file for initiative demo' \
  && printf '%s' "$got" | grep -q ' (cursor=unknown polls=0 last_ok=never)$' \
  && ! printf '%s' "$got" | grep -q 'hint:' && ! printf '%s' "$got" | LC_ALL=C grep -q "$(printf '\033')" \
  && [ "${#reason}" -le 200 ] && grep -q 'exit=4 configuration error' "$FAKE_DIR/loop.log" && ok=0
case_result "W3d exit 4 shows the first error line and the status" $ok "rc=$rc, reason length=${#reason}, line='$(printf '%s' "$got" | cut -c1-70)...'"

# W3e: the other direction: a stderr with ONLY the status line still shows it (no empty reason),
# and a stderr without a status line shows the reason alone, without brackets
new_case w3e; printf '3||\\ncursor=c1 polls=4 last_ok=2026-10-18T10:00:00Z\n' >"$FAKE_DIR/plan"
only_status=$("$W" --pause 0 2>/dev/null)
new_case w3f; printf '2||refused [rule=not-approved] claude@host-a is not approved\\nask the owner to run approve\n' >"$FAKE_DIR/plan"
only_reason=$("$W" --pause 0 2>/dev/null)
ok=1; [ "$only_status" = "crewboard-watch: exit 3: cursor=c1 polls=4 last_ok=2026-10-18T10:00:00Z" ] \
  && [ "$only_reason" = "crewboard-watch: exit 2: refused [rule=not-approved] claude@host-a is not approved" ] && ok=0
case_result "W3e status alone is shown; reason alone has no brackets" $ok "status-only='$only_status', reason-only='$only_reason'"

# W4: the env file is read by the binary (crewboard monitor), its token reaches the child, it is never
# printed; bad mode is refused (exit 4, one line, nothing runs)
new_case w4; echo '0|EVENT|' >"$FAKE_DIR/plan"
printf 'GH_TOKEN=tok-PLANTED-123\n' >"$T/ok.env"; chmod 600 "$T/ok.env"
"$W" --pause 0 --env-file "$T/ok.env" >"$FAKE_DIR/stdout" 2>"$FAKE_DIR/stderr"; rc=$?
seen=$(cat "$FAKE_DIR/seen-token")
leak=0; grep -q 'PLANTED' "$FAKE_DIR/stdout" "$FAKE_DIR/stderr" && leak=1
printf 'GH_TOKEN=x\n' >"$T/open.env"; chmod 644 "$T/open.env"
new_case w4b; echo '0|EVENT|' >"$FAKE_DIR/plan"
"$W" --pause 0 --env-file "$T/open.env" >"$FAKE_DIR/stdout" 2>/dev/null; rcb=$?
ok=1; [ "$seen" = "tok-PLANTED-123" ] && [ "$leak" -eq 0 ] && [ "$rcb" -eq 4 ] && [ "$(ncalls)" -eq 0 ] && ok=0
case_result "W4 env file passed, not printed; mode 644 refused" $ok "token reached crewboard=$([ "$seen" = tok-PLANTED-123 ] && echo yes || echo no), leaked=$leak, open-file rc=$rcb"

# W5: arguments: --config-dir, --timeout, --initiative reach crewboard
new_case w5; echo '0|EVENT|' >"$FAKE_DIR/plan"
"$W" --pause 0 --config-dir /x/cfg --timeout 7m --initiative demo >/dev/null 2>&1
c=$(head -1 "$FAKE_DIR/calls")
ok=1; [ "$c" = "--config-dir /x/cfg watch --timeout 7m --initiative demo" ] && ok=0
case_result "W5 arguments reach crewboard" $ok "call='$c'"

# W6: --log counts silent cycles
new_case w6; rep 5 '1||' >"$FAKE_DIR/plan"
"$W" --pause 0 --max-cycles 5 --log "$FAKE_DIR/loop.log" >/dev/null 2>&1
ok=1; [ "$(nlines "$FAKE_DIR/loop.log")" -eq 5 ] && tail -1 "$FAKE_DIR/loop.log" | grep -q 'silent=5' && ok=0
case_result "W6 --log counts 5 silent cycles" $ok "log lines=$(nlines "$FAKE_DIR/loop.log")"

# ---------------- poll-and-wake ----------------
echo "== poll-and-wake.sh"
WAKES() { nlines "$FAKE_DIR/wakes"; }
WAKE='echo wake >> "$FAKE_DIR/wakes"'

# P1: 20 polls, all exit 1 -> wake started 0 times, no stdout
new_case p1; rep 20 '1||' >"$FAKE_DIR/plan"
bytes=0; i=0
while [ $i -lt 20 ]; do
  o=$("$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" 2>/dev/null); r=$?
  bytes=$((bytes + ${#o})); [ $r -ne 0 ] && bytes=$((bytes + 1000))
  i=$((i + 1))
done
ok=1; [ "$(WAKES)" -eq 0 ] && [ "$bytes" -eq 0 ] && [ "$(ncalls)" -eq 20 ] && ok=0
case_result "P1 20x poll exit 1 -> 0 wakes, silent" $ok "wakes=$(WAKES), stdout bytes=$bytes, polls=$(ncalls)"

# P2: one exit 0 among them -> exactly one wake
new_case p2; { rep 7 '1||'; echo '0||'; rep 12 '1||'; } >"$FAKE_DIR/plan"
i=0; while [ $i -lt 20 ]; do "$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; i=$((i + 1)); done
ok=1; [ "$(WAKES)" -eq 1 ] && [ ! -d "$FAKE_DIR/lock" ] && ok=0
case_result "P2 one exit 0 among 20 -> exactly 1 wake, lock released" $ok "wakes=$(WAKES), lock left=$([ -d "$FAKE_DIR/lock" ] && echo yes || echo no)"

# P3: exit 3 -> non-zero, no wake, nothing on stdout, logged
new_case p3; echo '3||network down' >"$FAKE_DIR/plan"
o=$("$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 3 ] && [ "$(WAKES)" -eq 0 ] && [ -z "$o" ] && grep -q 'exit 3: network down' "$FAKE_DIR/p.log" && ok=0
case_result "P3 exit 3 -> rc 3, no wake, logged" $ok "rc=$rc, wakes=$(WAKES)"

# P3b: exit 4 passes through; a missing wake command is a usage error (64)
new_case p3b; echo '4||no key' >"$FAKE_DIR/plan"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc4=$?
env -u CREWBOARD_WAKE_CMD "$P" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rcn=$?
ok=1; [ "$rc4" -eq 4 ] && [ "$rcn" -eq 64 ] && ok=0
case_result "P3b exit 4 passes through; no wake command -> 64" $ok "rc=$rc4 and $rcn"

# P3d: a failing poll logs the code AND the reason: the FIRST stderr line, cut to 200 characters,
# without control characters. The fake prints a config error, then a hint line.
new_case p3d
printf '4||configuration error: no board file for initiative demo\033[31m %s\\nhint: run crewboard join first\n' \
  "$(rep 60 'x' | tr -d '\n')$(rep 300 'Z' | tr -d '\n')" >"$FAKE_DIR/plan"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" >/dev/null 2>"$FAKE_DIR/err"; rc=$?
line=$(grep 'STOPPED: crewboard poll exit 4' "$FAKE_DIR/p.log")
reason=${line#*exit 4: }; reason=${reason%%. No more wakes*}
ok=1; [ "$rc" -eq 4 ] && [ "$(WAKES)" -eq 0 ] \
  && printf '%s' "$line" | grep -q 'exit 4: configuration error: no board file for initiative demo' \
  && ! printf '%s' "$line" | grep -q 'hint:' && ! printf '%s' "$line" | LC_ALL=C grep -q "$(printf '\033')" \
  && [ "${#reason}" -le 200 ] && grep -q 'configuration error' "$FAKE_DIR/err" && ok=0
case_result "P3d failing poll logs its first stderr line, cut, no ctrl chars" $ok "rc=$rc, reason length=${#reason}"
# P3e: the other direction: a quiet poll (exit 1) logs nothing at all
new_case p3e '1||configuration error: never shown'
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" --work-backoff off >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ ! -s "$FAKE_DIR/p.log" ] && ok=0
case_result "P3e quiet poll -> nothing logged" $ok "rc=$rc, log lines=$(nlines "$FAKE_DIR/p.log")"

# Q1: quiet polls inside one hour write NO log line and nothing to stdout or stderr; they are
# counted in <log>.quiet (one line per hour at most, never one line per poll)
new_case q1; rep 5 '1||' >"$FAKE_DIR/plan"
i=0; out=""
while [ $i -lt 5 ]; do
  o=$("$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" 2>&1); out="$out$o"
  i=$((i + 1))
done
read -r q_start q_ts q_n <"$FAKE_DIR/p.log.quiet" 2>/dev/null
ok=1; [ ! -s "$FAKE_DIR/p.log" ] && [ -z "$out" ] && [ "$(WAKES)" -eq 0 ] && [ "$q_n" = 5 ] && ok=0
case_result "Q1 5 quiet polls inside the hour -> no log line, count 5" $ok "log lines=$(nlines "$FAKE_DIR/p.log"), output bytes=${#out}, count=$q_n"

# Q2: the first quiet poll after the hour writes ONE summary line with the right N (the 5 polls
# of Q1 plus this one), and starts a new window; the next quiet poll writes no line
printf '%s %s %s\n' "$(( $(date +%s) - 3601 ))" 2026-01-01T00:00:00Z "$q_n" >"$FAKE_DIR/p.log.quiet"
printf '1||\n1||\n' >>"$FAKE_DIR/plan"
o=$("$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" 2>&1); rc=$?
line=$(cat "$FAKE_DIR/p.log")
read -r q_start q_ts q_n <"$FAKE_DIR/p.log.quiet" 2>/dev/null
n_after=$q_n
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" >/dev/null 2>&1
read -r q_start q_ts q_n <"$FAKE_DIR/p.log.quiet" 2>/dev/null
ok=1; [ "$rc" -eq 0 ] && [ -z "$o" ] && [ "$(nlines "$FAKE_DIR/p.log")" -eq 1 ] \
  && printf '%s' "$line" | grep -Eq '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z poll-and-wake: quiet: 6 polls since 2026-01-01T00:00:00Z$' \
  && [ "$n_after" = 0 ] && [ "$q_n" = 1 ] && ok=0
case_result "Q2 first quiet poll after an hour -> one line, N=6" $ok "log lines=$(nlines "$FAKE_DIR/p.log"), line='$line', count after=$n_after, then=$q_n"

# Q3: the other direction: a wake still logs as before (its two lines, no quiet line), and a
# broken count file starts a new window instead of a wrong count
new_case q3 '1||' '0||' '1||'
printf 'junk\n' >"$FAKE_DIR/p.log.quiet"
i=0; while [ $i -lt 3 ]; do "$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" >/dev/null 2>&1; i=$((i + 1)); done
read -r q_start q_ts q_n <"$FAKE_DIR/p.log.quiet" 2>/dev/null
ok=1; [ "$(WAKES)" -eq 1 ] && [ "$(nlines "$FAKE_DIR/p.log")" -eq 2 ] \
  && grep -q 'poll-and-wake: events pending; waking: ' "$FAKE_DIR/p.log" \
  && grep -q 'poll-and-wake: wake finished OK$' "$FAKE_DIR/p.log" && ! grep -q 'quiet:' "$FAKE_DIR/p.log" \
  && [ "$q_n" = 2 ] && ok=0
case_result "Q3 a wake logs as before; broken count file restarts" $ok "wakes=$(WAKES), log lines=$(nlines "$FAKE_DIR/p.log"), quiet count=$q_n"

# P3c: a failing wake command -> exit 70, lock released
new_case p3c; echo '0||' >"$FAKE_DIR/plan"
"$P" --wake-cmd 'exit 9' --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 70 ] && [ ! -d "$FAKE_DIR/lock" ] && ok=0
case_result "P3c failing wake -> rc 70, lock released" $ok "rc=$rc"

# P4: lock held by a live wake -> a second poll-and-wake does not start a second wake
new_case p4; rep 5 '0||' >"$FAKE_DIR/plan"
SLOWWAKE='echo wake >> "$FAKE_DIR/wakes"; sleep 3'
"$P" --wake-cmd "$SLOWWAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" >/dev/null 2>&1 &
first=$!
i=0; while [ ! -s "$FAKE_DIR/lock/started" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
"$P" --wake-cmd "$SLOWWAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" >/dev/null 2>&1; rc2=$?
w_during=$(WAKES)
wait $first; rc1=$?
ok=1; [ "$w_during" -eq 1 ] && [ "$(WAKES)" -eq 1 ] && [ "$rc2" -eq 0 ] && [ "$rc1" -eq 0 ] \
  && grep -q 'already runs' "$FAKE_DIR/p.log" && [ ! -d "$FAKE_DIR/lock" ] && ok=0
case_result "P4 lock held -> second run starts no second wake" $ok "wakes=$(WAKES), second rc=$rc2, lock released after=$([ -d "$FAKE_DIR/lock" ] && echo no || echo yes)"

# P5: stale lock (dead pid) is taken over, and said so
new_case p5; echo '0||' >"$FAKE_DIR/plan"
sh -c 'exit 0' & dead=$!; wait $dead
mkdir "$FAKE_DIR/lock"; echo old-owner >"$FAKE_DIR/lock/owner"; echo "$dead" >"$FAKE_DIR/lock/pid"; date +%s >"$FAKE_DIR/lock/started"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(WAKES)" -eq 1 ] && grep -q 'STALE lock (pid-dead: owner pid .* is gone' "$FAKE_DIR/p.log" && ok=0
case_result "P5 stale lock (dead pid) taken over, wake runs once" $ok "wakes=$(WAKES), rc=$rc"

# P5b: lock of a LIVE pid that is older than the budget counts as stale; a young live lock does not
new_case p5b; rep 2 '0||' >"$FAKE_DIR/plan"
sleep 30 & live=$!
mkdir "$FAKE_DIR/lock"; echo o >"$FAKE_DIR/lock/owner"; echo "$live" >"$FAKE_DIR/lock/pid"; echo $(( $(date +%s) - 100 )) >"$FAKE_DIR/lock/started"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --stale-after 3600 --log "$FAKE_DIR/p.log" >/dev/null 2>&1
young=$(WAKES)
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --stale-after 10 --log "$FAKE_DIR/p.log" >/dev/null 2>&1
old=$(WAKES)
kill $live 2>/dev/null; wait $live 2>/dev/null
ok=1; [ "$young" -eq 0 ] && [ "$old" -eq 1 ] && ok=0
case_result "P5b live+young lock respected, live+over-budget taken" $ok "wakes after young=$young, after over-budget=$old"

# P5c: a FRESH empty lock directory (mkdir done, no pid file yet) is never taken over
new_case p5c; rep 2 '0||' >"$FAKE_DIR/plan"
mkdir "$FAKE_DIR/lock"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(WAKES)" -eq 0 ] && [ -d "$FAKE_DIR/lock" ] && grep -q 'already runs' "$FAKE_DIR/p.log" && ! grep -q STALE "$FAKE_DIR/p.log" && ok=0
case_result "P5c fresh empty lock dir is NOT taken over" $ok "wakes=$(WAKES), rc=$rc"

# P5d: an empty lock directory older than the grace time is taken over, with the named reason
touch -t 202001010000 "$FAKE_DIR/lock"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --empty-grace 5 --log "$FAKE_DIR/p.log" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(WAKES)" -eq 1 ] && grep -q 'STALE lock (empty-after-grace:' "$FAKE_DIR/p.log" && ok=0
case_result "P5d empty lock dir older than the grace is taken over" $ok "wakes=$(WAKES), rc=$rc"

# P6: --config-dir reaches the wake command as CREWBOARD_CONFIG_DIR (the woken agent must use the same config)
new_case p6; echo '0||' >"$FAKE_DIR/plan"
"$P" --config-dir "$T/agentcfg" --wake-cmd 'printf "%s" "$CREWBOARD_CONFIG_DIR" > "$FAKE_DIR/cfgseen"' --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(cat "$FAKE_DIR/cfgseen" 2>/dev/null)" = "$T/agentcfg" ] && ok=0
case_result "P6 --config-dir is passed to the wake command" $ok "rc=$rc, seen=$(cat "$FAKE_DIR/cfgseen" 2>/dev/null)"

# P7: no events, but the agent owns an In Progress or Ready ticket (`mine` exit 0), no wake yet
# -> one wake for the work, the wake command knows the reason, the back-off file is written
new_case p7; echo '1||' >"$FAKE_DIR/plan"; echo 0 >"$FAKE_DIR/plan.mine"
"$P" --wake-cmd 'echo wake >> "$FAKE_DIR/wakes"; printf "%s" "$CREWBOARD_WAKE_REASON" > "$FAKE_DIR/reason"' \
  --lock-dir "$FAKE_DIR/lock" --log "$FAKE_DIR/p.log" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(WAKES)" -eq 1 ] && [ "$(cat "$FAKE_DIR/reason")" = "work" ] \
  && [ -s "$FAKE_DIR/last-wake" ] && grep -q 'owns an In Progress or Ready ticket' "$FAKE_DIR/p.log" && ok=0
case_result "P7 owned work, no events, no earlier wake -> 1 work wake" $ok "wakes=$(WAKES), reason=$(cat "$FAKE_DIR/reason" 2>/dev/null), rc=$rc"

# P8: no events and NO owned ticket (`mine` exit 1) -> no wake, silent (the other direction of P7)
new_case p8; echo '1||' >"$FAKE_DIR/plan"; echo 1 >"$FAKE_DIR/plan.mine"
o=$("$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(WAKES)" -eq 0 ] && [ -z "$o" ] && [ "$(nlines "$FAKE_DIR/calls.mine")" -eq 1 ] && ok=0
case_result "P8 no events, no owned ticket -> no wake, silent" $ok "wakes=$(WAKES), stdout bytes=${#o}, mine calls=$(nlines "$FAKE_DIR/calls.mine")"

# P9: owned work, but the last wake was 1 minute ago (back-off 900 s) -> no wake; `mine` is not even asked
new_case p9; echo '1||' >"$FAKE_DIR/plan"; echo 0 >"$FAKE_DIR/plan.mine"
echo $(( $(date +%s) - 60 )) >"$FAKE_DIR/last-wake"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(WAKES)" -eq 0 ] && [ "$(nlines "$FAKE_DIR/calls.mine")" -eq 0 ] && ok=0
case_result "P9 back-off not elapsed -> no wake" $ok "wakes=$(WAKES), mine calls=$(nlines "$FAKE_DIR/calls.mine")"

# P10: the same state, last wake 1000 s ago (over the 900 s back-off) -> wake, and the back-off restarts
new_case p10; echo '1||' >"$FAKE_DIR/plan"; echo 0 >"$FAKE_DIR/plan.mine"
old=$(( $(date +%s) - 1000 )); echo "$old" >"$FAKE_DIR/last-wake"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
now_lw=$(cat "$FAKE_DIR/last-wake")
ok=1; [ "$rc" -eq 0 ] && [ "$(WAKES)" -eq 1 ] && [ "$now_lw" -gt "$old" ] && ok=0
case_result "P10 back-off elapsed -> work wake, back-off restarts" $ok "wakes=$(WAKES), last-wake moved=$([ "$now_lw" -gt "$old" ] && echo yes || echo no)"

# P10b: --work-backoff is configurable (a 5 s back-off, last wake 10 s ago -> wake), and `off` never wakes for work
new_case p10b; echo '1||' >"$FAKE_DIR/plan"; echo 0 >"$FAKE_DIR/plan.mine"
echo $(( $(date +%s) - 10 )) >"$FAKE_DIR/last-wake"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --work-backoff 5 >/dev/null 2>&1
w5=$(WAKES)
rm -f "$FAKE_DIR/last-wake"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --work-backoff off >/dev/null 2>&1
woff=$(WAKES)
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --work-backoff soon >/dev/null 2>&1; rcb=$?
ok=1; [ "$w5" -eq 1 ] && [ "$woff" -eq 1 ] && [ "$rcb" -eq 64 ] && ok=0
case_result "P10b --work-backoff 5 wakes, off does not, junk -> 64" $ok "wakes after 5=$w5, after off=$woff, junk rc=$rcb"

# P11: an event wake starts the back-off too (a quiet tick right after it does not wake for work)
new_case p11; { echo '0||'; echo '1||'; } >"$FAKE_DIR/plan"; echo 0 >"$FAKE_DIR/plan.mine"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1
ok=1; [ "$(WAKES)" -eq 1 ] && ok=0
case_result "P11 event wake starts the back-off: quiet tick after it -> no 2nd wake" $ok "wakes=$(WAKES)"

# P12: during a long wake a heartbeat goes out every --heartbeat-every seconds, and stops with the wake
new_case p12; echo '0||' >"$FAKE_DIR/plan"
"$P" --wake-cmd 'sleep 4' --lock-dir "$FAKE_DIR/lock" --heartbeat-every 1 >/dev/null 2>&1; rc=$?
hb1=$(nlines "$FAKE_DIR/calls.hb")
sleep 2
hb2=$(nlines "$FAKE_DIR/calls.hb")
ok=1; [ "$rc" -eq 0 ] && [ "$hb1" -ge 2 ] && [ "$hb2" -eq "$hb1" ] && ok=0
case_result "P12 heartbeat every 1 s during a 4 s wake, none after it" $ok "heartbeats during=$hb1, 2 s after=$hb2, rc=$rc"

# P12b: a fast wake sends none (the interval is longer than the wake); --heartbeat-every 0 sends none at all
new_case p12b; echo '0||' >"$FAKE_DIR/plan"
"$P" --wake-cmd 'true' --lock-dir "$FAKE_DIR/lock" --heartbeat-every 60 >/dev/null 2>&1
fast=$(nlines "$FAKE_DIR/calls.hb")
new_case p12c; echo '0||' >"$FAKE_DIR/plan"
"$P" --wake-cmd 'sleep 2' --lock-dir "$FAKE_DIR/lock" --heartbeat-every 0 >/dev/null 2>&1
zero=$(nlines "$FAKE_DIR/calls.hb")
ok=1; [ "$fast" -eq 0 ] && [ "$zero" -eq 0 ] && ok=0
case_result "P12b fast wake and --heartbeat-every 0 -> no heartbeat" $ok "fast=$fast, zero=$zero"

# ---------------- with-env ----------------
echo "== crewboard agent with-env"
new_case e1; echo '0||' >"$FAKE_DIR/plan"
o=$(crewboard agent with-env --env-file "$T/ok.env" -- crewboard take 12 2>&1); rc=$?
seen=$(cat "$FAKE_DIR/seen-token")
crewboard agent with-env --env-file "$T/open.env" -- crewboard take 12 >/dev/null 2>&1; rco=$?
ok=1; [ "$rc" -eq 0 ] && [ "$seen" = "tok-PLANTED-123" ] && [ -z "$o" ] && [ "$rco" -eq 4 ] && ok=0
case_result "E1 agent with-env passes token silently; mode 644 refused" $ok "token reached crewboard=$([ "$seen" = tok-PLANTED-123 ] && echo yes || echo no), printed bytes=${#o}, open-file rc=$rco"
# E1b: an env file without GH_TOKEN, or one that does not load, runs nothing (fail closed)
new_case e1b; echo '0||' >"$FAKE_DIR/plan"
printf 'OTHER=1\n' >"$T/notoken.env"; chmod 600 "$T/notoken.env"
printf 'GH_TOKEN="tok-broken\n' >"$T/broken.env"; chmod 600 "$T/broken.env"
o=$(crewboard agent with-env --env-file "$T/notoken.env" -- crewboard take 12 2>&1); rc1=$?
o2=$(crewboard agent with-env --env-file "$T/broken.env" -- crewboard take 12 2>&1); rc2=$?
ok=1; [ "$rc1" -eq 4 ] && [ "$rc2" -eq 4 ] && [ "$(ncalls)" -eq 0 ] && printf '%s' "$o" | grep -q 'has no GH_TOKEN or GITHUB_TOKEN' && printf '%s' "$o2" | grep -q 'line 1' && ok=0
case_result "E1b env file without a token or broken: exit 4, nothing run" $ok "rc=$rc1/$rc2, calls=$(ncalls)"

# ---------------- gh-shim ----------------
echo "== crewboard agent shim"
# the shim commands of the binary: the agent dir is the parent of the shim dir
mkshim() { # --dir <bin dir> --env-file <file> [--real-gh <path>]
  local dir="" envf="" real=""
  while [ $# -gt 0 ]; do case "$1" in --dir) dir=$2 ;; --env-file) envf=$2 ;; --real-gh) real=$2 ;; esac; shift 2; done
  set -- agent shim --agent-dir "$(dirname "$dir")" --bin-dir "$dir" --env-file "$envf"
  [ -z "$real" ] || set -- "$@" --real-gh "$real"
  crewboard "$@"
}
gitcfg() { # --repo <dir> --shim <gh stub> --crew-login <login> --email <email>
  local repo="" shim="" login="" email=""
  while [ $# -gt 0 ]; do case "$1" in --repo) repo=$2 ;; --shim) shim=$2 ;; --crew-login) login=$2 ;; --email) email=$2 ;; esac; shift 2; done
  crewboard agent shim --only-git --agent-dir "$(dirname "$(dirname "$shim")")" --bin-dir "$(dirname "$shim")" \
    --work-dir "$repo" --crew-login "$login" --email "$email"
}
GS="$T/gh-shim"; mkdir -p "$GS/real" "$GS/agent/bin"
# crewboard writes a path without a doubled slash, which $TMPDIR can give
GSN=$(printf '%s' "$GS" | sed 's#//*#/#g')
# a fake real gh: says which token it saw and which config dir it got
cat >"$GS/real/gh" <<'STUB'
#!/bin/bash
echo "gh-saw token=${GH_TOKEN:-unset} args=$*"
STUB
chmod +x "$GS/real/gh"
printf 'GH_TOKEN=tok-CREW-456\n' >"$GS/crew.env"; chmod 600 "$GS/crew.env"

# G1: make-shim writes a shim that loads the agent token for the real gh; a bare gh sees none
out=$(mkshim --dir "$GS/agent/bin" --env-file "$GS/crew.env" --real-gh "$GS/real/gh" 2>&1); rc=$?
via=$(env -u GH_TOKEN "$GS/agent/bin/gh" api user --jq .login 2>&1)
bare=$(env -u GH_TOKEN "$GS/real/gh" api user 2>&1)
ok=1; [ "$rc" -eq 0 ] && [ "$via" = "gh-saw token=tok-CREW-456 args=api user --jq .login" ] \
  && [ "$bare" = "gh-saw token=unset args=api user" ] && [ -d "$GS/agent/gh-empty-config" ] \
  && printf '%s' "$out" | grep -q 'agent shim: \[1/1\] gh shim: done ' && ok=0
case_result "G1 shim loads the agent token; a bare gh has none" $ok "rc=$rc, via shim='$via', bare='$bare'"

# G1b: the token never appears in the shim file or in the printed lines
leak=0; grep -q 'tok-CREW-456' "$GS/agent/bin/gh" && leak=1; printf '%s' "$out" | grep -q 'tok-CREW-456' && leak=1
case_result "G1b token is not written into the shim or printed" $leak "leak=$([ $leak -eq 0 ] && echo no || echo YES)"

# G1c: without --real-gh the first gh on the PATH is used, never the shim itself
mkdir -p "$GS/agent2/bin"
out=$(PATH="$GS/agent2/bin:$GS/real:$PATH" mkshim --dir "$GS/agent2/bin" --env-file "$GS/crew.env" 2>&1); rc=$?
via=$(env -u GH_TOKEN "$GS/agent2/bin/gh" version 2>&1)
ok=1; [ "$rc" -eq 0 ] && [ "$via" = "gh-saw token=tok-CREW-456 args=version" ] && ok=0
case_result "G1c real gh found on the PATH, not the shim" $ok "rc=$rc, via='$via'"

# G1d: a missing env file is refused (exit 4)
mkshim --dir "$GS/agent3" --env-file "$GS/none.env" --real-gh "$GS/real/gh" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 4 ] && ok=0
case_result "G1d missing env file -> rc 4" $ok "rc=$rc"

# G1e: for <agent-dir>/bin the next step names `agent exec`, which finds the shim only there. A shim dir
# that is not <agent-dir>/bin gets no such line.
out=$(mkshim --dir "$GS/agent/bin" --env-file "$GS/crew.env" --real-gh "$GS/real/gh" 2>&1)
other=$(mkshim --dir "$GS/agent5/shims" --env-file "$GS/crew.env" --real-gh "$GS/real/gh" 2>&1)
ok=1; printf '%s' "$out" | grep -qF "next step: start the runtime with the shim first on its PATH: crewboard agent exec --agent-dir '$GSN/agent' -- <runtime>" \
  && ! printf '%s' "$other" | grep -q 'agent exec' && [ -x "$GS/agent5/shims/gh" ] && [ -d "$GS/agent5/gh-empty-config" ] && ok=0
case_result "G1e make-shim names agent exec only for <agent-dir>/bin" $ok "exec line for bin=$(printf '%s' "$out" | grep -c 'agent exec'), for shims=$(printf '%s' "$other" | grep -c 'agent exec')"

# G2: git-config in a LINKED WORKTREE changes the worktree only; the main checkout keeps its identity
GIT=(git -c init.defaultBranch=main)
mkdir -p "$GS/owner" && "${GIT[@]}" -C "$GS/owner" init -q 2>/dev/null
git -C "$GS/owner" config user.email owner@example.org; git -C "$GS/owner" config user.name owner
git -C "$GS/owner" -c user.email=o@x -c user.name=o commit -q --allow-empty -m init
git -C "$GS/owner" worktree add -q -b crewwt "$GS/wt" 2>/dev/null
gitcfg --repo "$GS/wt" --shim "$GS/agent/bin/gh" --crew-login crew-bot --email 'crew@users.noreply.example' >/dev/null 2>&1; rc=$?
wt_mail=$(git -C "$GS/wt" config user.email)
main_mail=$(git -C "$GS/owner" config user.email)
wt_inst=$(git -C "$GS/wt" config --get url.https://github.com/.insteadOf)
main_inst=$(git -C "$GS/owner" config --get url.https://github.com/.insteadOf)
helpers=$(git -C "$GS/wt" config --get-all credential.helper | tr '\n' '|')
ok=1; [ "$rc" -eq 0 ] && [ "$wt_mail" = "crew@users.noreply.example" ] && [ "$main_mail" = "owner@example.org" ] \
  && [ "$wt_inst" = "git@github.com:" ] && [ -z "$main_inst" ] && ok=0
# the empty entry resets the helpers of the system and the user; ours comes after it. A host
# without inherited helpers gives "|!<shim>...|" (Linux CI), a host with one gives
# "<inherited>||!<shim>...|" (macOS keychain). Both are right; anything else is not.
case "$helpers" in "|!$GSN/agent/bin/gh auth git-credential|"|*"||!$GSN/agent/bin/gh auth git-credential|") ;; *) ok=1 ;; esac
case_result "G2 worktree gets the crew git config, the owner checkout none" $ok "wt=$wt_mail main=$main_mail insteadOf(wt)=$wt_inst helpers=$helpers"

# G3: git-config in a plain clone sets local config
mkdir -p "$GS/clone" && "${GIT[@]}" -C "$GS/clone" init -q 2>/dev/null
gitcfg --repo "$GS/clone" --shim "$GS/agent/bin/gh" --crew-login crew-bot --email 'crew@users.noreply.example' >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(git -C "$GS/clone" config --local user.email)" = "crew@users.noreply.example" ] && ok=0
case_result "G3 plain clone gets local crew git config" $ok "rc=$rc"

# ---------------- agent-wake ----------------
echo "== agent-wake.sh"
A="$DRV/agent-wake.sh"
AWX="$A"
if [ "${CB_IMPL:-shell}" = rust ]; then
  # CB_IMPL=rust: agent-wake.sh is `crewboard wake` with the options of the dev agent. The wrapper maps
  # --role coordinator to --tick, drops --role dev and adds the fake board unless the case passes its own --crewboard.
  AWX="$T/bin/aw"
  cat >"$AWX" <<EOF_AW
#!/bin/sh
export CREWBOARD_CONFIG_DIR="\${CREWBOARD_CONFIG_DIR:-$T/cfg-none}"
seam=1; n=\$#
while [ "\$n" -gt 0 ]; do
  a=\$1; shift; n=\$((n - 1))
  case "\$a" in
    --role) r=\$1; shift; n=\$((n - 1)); [ "\$r" != coordinator ] || set -- "\$@" --tick ;;
    --crewboard) v=\$1; shift; n=\$((n - 1)); seam=0; set -- "\$@" --crewboard "\$v" ;;
    *) set -- "\$@" "\$a" ;;
  esac
done
[ "\$seam" = 0 ] || set -- "\$@" --crewboard '$T/bin/crewboard'
exec '$CREWBOARD_REAL' wake "\$@"
EOF_AW
  chmod +x "$AWX"
fi
AW="$T/agent-wake"; mkdir -p "$AW/wd" "$AW/agent/bin" "$AW/agent/gh-empty-config" "$AW/real"
printf '#!/bin/bash\nexit 0\n' >"$AW/agent/bin/gh"; chmod +x "$AW/agent/bin/gh"
printf 'GH_TOKEN=test-not-real\n' >"$AW/env"; chmod 600 "$AW/env"
# fake runtimes: record cwd, PATH head, GH_CONFIG_DIR and the arguments, one word per line
for rt in claude codex hermes; do
  cat >"$T/bin/$rt" <<'RT'
#!/bin/bash
{
  echo "cwd=$(pwd -P)"
  echo "pathhead=${PATH%%:*}"
  echo "ghcfg=${GH_CONFIG_DIR:-unset}"
  echo "reason=${CREWBOARD_WAKE_REASON:-unset}"
  echo "events=${CREWBOARD_EVENTS_FILE:-unset}"
  echo "wake=${CREWBOARD_WAKE:-unset}"
  echo "lockdirs=${CREWBOARD_LOCK_RUN_DIRS-unset}"
  for a in "$@"; do echo "arg=$a"; done
} >"$FAKE_DIR/rt.out"
exit 0
RT
  chmod +x "$T/bin/$rt"
done
AWC=(--agent 'dev@host-a' --initiative demo --env-file "$AW/env" --work-dir "$AW/wd" --heartbeat-every 0 --work-backoff off)

# A1: claude, poll exit 0 -> the fake claude runs once, in the work dir, with -p, the permission flags and the prompt
new_case a1 '0||'
"$AWX" --runtime claude "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
wdreal=$(cd "$AW/wd" && pwd -P)
ok=1; [ "$rc" -eq 0 ] && grep -qx "cwd=$wdreal" "$FAKE_DIR/rt.out" && grep -qx 'arg=-p' "$FAKE_DIR/rt.out" \
  && grep -qx 'arg=dontAsk' "$FAKE_DIR/rt.out" && grep -q '^arg=Use the crewboard skill (SKILL.md and references/dev.md). You are dev@host-a, a dev agent of demo' "$FAKE_DIR/rt.out" \
  && grep -qx 'reason=events' "$FAKE_DIR/rt.out" \
  && [ "$(grep -B1 '^arg=Use the crewboard skill' "$FAKE_DIR/rt.out" | head -n 1)" = 'arg=--' ] && ok=0
case_result "A1 claude wake: cwd, -p, permission flags, -- before the prompt, prompt" $ok "rc=$rc"
# A1b: the default allow list has the shell helpers of compound commands (echo, test, ...)
al=$(grep '^arg=Bash(crewboard' "$FAKE_DIR/rt.out")
ok=1; case "$al" in *'Bash(echo *)'*'Bash(test *)'*'Bash(grep *)'*) ok=0 ;; esac
case_result "A1b claude default allow list: crewboard, git, gh and the shell helpers" $ok "${al:0:120}"

# A2: codex gets exec, the sandbox flags and the prompt
new_case a2 '0||'
"$AWX" --runtime codex "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -qx 'arg=exec' "$FAKE_DIR/rt.out" && grep -qx 'arg=workspace-write' "$FAKE_DIR/rt.out" \
  && grep -q '^arg=Use the crewboard skill (SKILL.md and references/dev.md). You are dev@host-a' "$FAKE_DIR/rt.out" && ok=0
case_result "A2 codex wake: exec, sandbox flags, prompt" $ok "rc=$rc"

# A3: a quiet board starts NO runtime (the other direction of A1)
new_case a3 '1||'
rm -f "$FAKE_DIR/rt.out"
"$AWX" --runtime claude "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ ! -f "$FAKE_DIR/rt.out" ] && ok=0
case_result "A3 quiet board -> no runtime started" $ok "rc=$rc, runtime ran=$([ -f "$FAKE_DIR/rt.out" ] && echo yes || echo no)"

# A4: --shim-dir puts the shim first on PATH and sets the empty gh config dir; without it neither is set
new_case a4 '0||'
"$AWX" --runtime claude "${AWC[@]}" --shim-dir "$AW/agent/bin" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
shimreal=$(cd "$AW/agent/bin" && pwd); cfgreal=$(cd "$AW/agent/gh-empty-config" && pwd)
ok=1; [ "$rc" -eq 0 ] && grep -qx "pathhead=$shimreal" "$FAKE_DIR/rt.out" && grep -qx "ghcfg=$cfgreal" "$FAKE_DIR/rt.out" && ok=0
case_result "A4 --shim-dir: shim first on PATH, empty GH_CONFIG_DIR" $ok "rc=$rc"
new_case a4b '0||'
"$AWX" --runtime claude "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1
ok=1; grep -qx 'ghcfg=unset' "$FAKE_DIR/rt.out" && ok=0
case_result "A4b no --shim-dir: GH_CONFIG_DIR stays unset" $ok "$(grep ghcfg "$FAKE_DIR/rt.out")"

# A5: --runtime-flags replaces the defaults; --prompt-extra and a quote in the prompt survive sh -c
new_case a5 '0||'
"$AWX" --runtime claude "${AWC[@]}" --runtime-flags '--permission-mode acceptEdits' --prompt-extra "Don't stop." --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -qx 'arg=acceptEdits' "$FAKE_DIR/rt.out" && ! grep -qx 'arg=dontAsk' "$FAKE_DIR/rt.out" \
  && grep -q "Don't stop\.$" "$FAKE_DIR/rt.out" \
  && [ "$(grep -B1 '^arg=Use the crewboard skill' "$FAKE_DIR/rt.out" | head -n 1)" = 'arg=--' ] && ok=0
case_result "A5 --runtime-flags replace defaults; quote in prompt survives" $ok "rc=$rc"
# A5b: --model goes to the runtime as --model ID, for each runtime; without it no --model
for rt in claude codex hermes; do
  new_case a5b-$rt '0||'
  "$AWX" --runtime $rt "${AWC[@]}" --model small-model-1 --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
  ok=1; [ "$rc" -eq 0 ] && [ "$(grep -A1 -x 'arg=--model' "$FAKE_DIR/rt.out" | tail -n 1)" = 'arg=small-model-1' ] && ok=0
  case_result "A5b $rt: --model ID reaches the runtime" $ok "rc=$rc"
done
new_case a5c '0||'
"$AWX" --runtime claude "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && ! grep -qx 'arg=--model' "$FAKE_DIR/rt.out" && ok=0
case_result "A5c no --model: the runtime default applies" $ok "rc=$rc"

# A6: refusals exit 4 and start nothing: unknown runtime, missing work dir, shim dir without a shim
new_case a6 '0||'
rm -f "$FAKE_DIR/rt.out"
"$AWX" --runtime copilot "${AWC[@]}" >/dev/null 2>&1; r1=$?
"$AWX" --runtime claude --agent x --initiative demo --work-dir "$AW/nope" >/dev/null 2>&1; r2=$?
"$AWX" --runtime claude "${AWC[@]}" --shim-dir "$AW/real" >/dev/null 2>&1; r3=$?
ok=1; [ "$r1" -eq 4 ] && [ "$r2" -eq 4 ] && [ "$r3" -eq 4 ] && [ ! -f "$FAKE_DIR/rt.out" ] && ok=0
case_result "A6 bad runtime, work dir, shim dir -> exit 4, nothing started" $ok "rc=$r1,$r2,$r3"

# ---------------- agent dir (agent.env, agent env) ----------------
echo "== agent dir"
CAD="$T/ca"; mkdir -p "$CAD"
printf 'GH_TOKEN=crew-token-value\n' >"$CAD/crew.env"; chmod 600 "$CAD/crew.env"
# an agent dir: the config dir of one agent, with agent.env linked to the env file of the agent account
mkdir -p "$CAD/dev1" && ln -sfn "$CAD/crew.env" "$CAD/dev1/agent.env"
mkdir -p "$CAD/empty"

# C3: `agent env` prints the export lines of the agent shell: the shim dir first on PATH, the empty gh
# config dir, the config dir. Never the token. eval of the output sets them.
new_case c3
mkdir -p "$CAD/dev1/bin" "$CAD/dev1/gh-empty-config"
crewboard --config-dir "$CAD/dev1" agent env >"$FAKE_DIR/env.out" 2>&1; rc=$?
seen=$(PATH=/usr/bin:/bin; eval "$(cat "$FAKE_DIR/env.out")"; printf '%s|%s|%s' "${PATH%%:*}" "$GH_CONFIG_DIR" "$CREWBOARD_CONFIG_DIR")
dev1=$(cd "$CAD/dev1" && pwd)
ok=1; [ "$rc" -eq 0 ] && [ "$seen" = "$dev1/bin|$dev1/gh-empty-config|$dev1" ] && [ "$(nlines "$FAKE_DIR/env.out")" -eq 3 ] \
  && ! grep -q 'crew-token-value' "$FAKE_DIR/env.out" && [ "$(ncalls)" -eq 0 ] && ok=0
case_result "C3 agent env: shim first, empty gh config, config dir; no token" $ok "rc=$rc, seen=$seen"
# C3b: the other direction: without bin/ and gh-empty-config/ only CREWBOARD_CONFIG_DIR is printed;
# a missing agent dir exits 4 (was 64) and prints no export line
mkdir -p "$CAD/bare"
crewboard --config-dir "$CAD/bare" agent env >"$FAKE_DIR/bare.out" 2>&1; rb=$?
crewboard --config-dir "$CAD/nope" agent env >"$FAKE_DIR/nope.out" 2>/dev/null; rn=$?
ok=1; [ "$rb" -eq 0 ] && [ "$(cat "$FAKE_DIR/bare.out")" = "export CREWBOARD_CONFIG_DIR='$(cd "$CAD/bare" && pwd)'" ] \
  && [ "$rn" -eq 4 ] && [ ! -s "$FAKE_DIR/nope.out" ] && ok=0
case_result "C3b agent env without shim dirs: config dir only; no dir -> 4" $ok "rc=$rb,$rn, out=$(cat "$FAKE_DIR/bare.out")"

# C4: `crewboard --config-dir D monitor` loads the token of agent.env and gives it to the child that
# runs watch. One event -> printed, exit 0; the agent log is the --log of the caller. The wrapper adds the test seam --crewboard STUB
# after `monitor`, so that the real binary runs the fake as its watch.
cat >"$T/bin/cb-seam" <<'SEAM'
#!/bin/bash
args=(); injected=0
for a in "$@"; do
  args+=("$a")
  if [ "$injected" = 0 ] && [ "$a" = monitor ]; then args+=(--crewboard "$T/bin/crewboard"); injected=1; fi
done
exec env CREWBOARD_CONFIG_DIR="${CREWBOARD_CONFIG_DIR:-$T/cfg-none}" "$CREWBOARD_REAL" "${args[@]}"
SEAM
chmod +x "$T/bin/cb-seam"
new_case c4 '1||' '0|EVENT 1: assign #12|'
"$T/bin/cb-seam" --config-dir "$CAD/dev1" monitor --log "$CAD/dev1/watch-loop.log" --pause 0 --timeout 1m >"$FAKE_DIR/out" 2>&1; rc=$?
c1=$(head -n 1 "$FAKE_DIR/calls")
ok=1; [ "$rc" -eq 0 ] && [ "$(cat "$FAKE_DIR/out")" = "EVENT 1: assign #12" ] \
  && [ "$c1" = "--config-dir $CAD/dev1 watch --timeout 1m" ] && [ "$(cat "$FAKE_DIR/seen-token")" = "crew-token-value" ] \
  && grep -q 'silent=1 exit=1' "$CAD/dev1/watch-loop.log" && ok=0
case_result "C4 monitor: agent token, config dir, agent log, event printed" $ok "rc=$rc, call=$c1"
new_case c4b '0|EVENT|'
"$T/bin/cb-seam" --config-dir "$CAD/dev1" monitor --pause 0 --log "$FAKE_DIR/mine.log" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ -s "$FAKE_DIR/mine.log" ] && ok=0
case_result "C4b monitor: --log is the log file" $ok "rc=$rc, own log lines=$(nlines "$FAKE_DIR/mine.log")"
# ---------------- agent exec ----------------
# `agent exec` runs one command with the agent environment, from any shell, without eval.
echo "== crewboard agent exec"
# XE1: the command gets the shim first on PATH, the empty gh config dir and the config dir; the
# token variables of the caller (here an owner token) are gone; the exit code passes through
new_case x1
dev1=$(cd "$CAD/dev1" && pwd)
seen=$(env -i HOME="$T" PATH=/usr/bin:/bin GH_TOKEN=owner-token GITHUB_TOKEN=owner-token2 "$CREWBOARD_REAL" --config-dir "$CAD/dev1" agent exec -- \
  /bin/sh -c 'printf "%s|%s|%s|%s|%s" "${PATH%%:*}" "$GH_CONFIG_DIR" "$CREWBOARD_CONFIG_DIR" "${GH_TOKEN:-unset}" "${GITHUB_TOKEN:-unset}"'); rc=$?
crewboard --config-dir "$CAD/dev1" agent exec -- /bin/sh -c 'exit 7'; rc7=$?
ok=1; [ "$rc" -eq 0 ] && [ "$seen" = "$dev1/bin|$dev1/gh-empty-config|$dev1|unset|unset" ] && [ "$rc7" -eq 7 ] && ok=0
case_result "XE1 agent exec: agent exports set, caller token gone, exit code kept" $ok "rc=$rc,$rc7, seen=$seen"
# XE1b: the other direction: a missing agent dir exits 4, a missing command exits 64 (usage), and nothing runs
crewboard --config-dir "$CAD/nope" agent exec -- /bin/sh -c ": >'$FAKE_DIR/ran'" >/dev/null 2>&1; r1=$?
crewboard --config-dir "$CAD/dev1" agent exec -- >/dev/null 2>&1; r2=$?
ok=1; [ "$r1" -eq 4 ] && [ "$r2" -eq 64 ] && [ ! -e "$FAKE_DIR/ran" ] && ok=0
case_result "XE1b agent exec without agent dir -> 4, without command -> 64, nothing runs" $ok "rc=$r1,$r2"

# ---------------- agent add ----------------
echo "== crewboard agent add"
BOOT="$(cd "$T" && pwd)/boot"; mkdir -p "$BOOT/realgh" "$BOOT/remote/demo-owner"
printf 'GH_TOKEN=boot-token-SECRET\n' >"$BOOT/crew.env"; chmod 600 "$BOOT/crew.env"
# a fake crewboard: `join` writes the agent config like the real one; `start` answers as
# $FAKE_START says (pending, ok, other). It records the calls and, per call, the cwd, the config
# dir, the PATH head and the token it got.
cat >"$BOOT/crewboard" <<'STUB'
#!/bin/bash
D="$FAKE_DIR"
# `agent with-env` loads the env file and runs this fake again with the token.
# `agent add` asks this fake for `join`, `start` and the crew id (the hidden seam --crewboard)
if [ "$1" = agent ]; then
  if [ "$2" = add ]; then
    exec env CREWBOARD_CONFIG_DIR="${CREWBOARD_CONFIG_DIR:-$T/cfg-none}" "$CREWBOARD_REAL" agent add --crewboard "$0" "${@:3}"
  fi
  exec env CREWBOARD_CONFIG_DIR="${CREWBOARD_CONFIG_DIR:-$T/cfg-none}" "$CREWBOARD_REAL" "$@"
fi
printf '%s\n' "$*" >>"$D/calls"
printf '%s|%s|%s|%s\n' "$PWD" "${CREWBOARD_CONFIG_DIR:-unset}" "${PATH%%:*}" "${GH_TOKEN:-unset}" >>"$D/ctx"
cfg=""; name=""; prev=""
for a in "$@"; do
  [ "$prev" = "--config-dir" ] && cfg="$a"; [ "$prev" = "--name" ] && name="$a"; prev="$a"
done
case " $* " in
  *" join "*)
    [ "${FAKE_JOIN_RC:-0}" -eq 0 ] || { echo "configuration error: fake join failed" >&2; exit "$FAKE_JOIN_RC"; }
    printf 'name = "%s"\nruntime = "claude-code"\n' "$name" >"$cfg/demo.toml"
    echo "join request posted: http://127.0.0.1/registry"; exit 0 ;;
  *" start "*)
    case "${FAKE_START:-pending}" in
      ok) echo "state: active"; exit 0 ;;
      pending) echo "refused [rule=sender-not-approved]: $name is not an approved agent: the owner has not approved it yet" >&2; exit 2 ;;
      *) echo "refused [rule=agent-gh-not-crew]: plain gh acts as the owner" >&2; exit 2 ;;
    esac ;;
esac
exit 0
STUB
chmod +x "$BOOT/crewboard"
# a fake real gh: the crew login and its id, only with the agent token loaded (through the shim)
cat >"$BOOT/realgh/gh" <<'STUB'
#!/bin/bash
[ "$GH_TOKEN" = boot-token-SECRET ] || { echo "gh: not logged in" >&2; exit 4; }
case "$*" in
  "api user --jq .login") echo crew-bot ;;
  "api users/crew-bot --jq .id") echo 4242 ;;
  *) echo "fake gh: no answer for: $*" ;;
esac
STUB
chmod +x "$BOOT/realgh/gh"
# the clone source: https://github.com/<repo> maps to a local repo (git's own url.insteadOf)
"${GIT[@]}" -C "$BOOT/remote/demo-owner" init -q demo-repo 2>/dev/null
git -C "$BOOT/remote/demo-owner/demo-repo" -c user.email=o@x -c user.name=o commit -q --allow-empty -m init
printf '[url "%s/"]\n\tinsteadOf = https://github.com/\n' "$BOOT/remote" >"$BOOT/gitconfig"
BD="$BOOT/agents/dev1"; mkdir -p "$BOOT/agents"
bs() { # the `agent add` command of the cases; extra args come last
  PATH="$BOOT/realgh:$PATH" GIT_CONFIG_GLOBAL="$BOOT/gitconfig" GIT_CONFIG_NOSYSTEM=1 \
    "$BOOT/crewboard" agent add --agent-dir "$1" --env-file "$BOOT/crew.env" --agent dev1@host-b --runtime claude \
    --repo demo-owner/demo-repo --work-dir "$2" "${@:3}"
}
steps() { grep -c "^agent add: \[[1-7]/7\] [a-z ]*: $2" "$1"; }

# B1: the first run: every step done, join posted, start not tried, one next step (approval)
new_case b1
bs "$BD" "$BOOT/work" >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
wt="$BOOT/work"; bd=$(cd "$BD" && pwd)
next=$(grep -c '^agent add: next step:' "$FAKE_DIR/out")
ok=1; [ "$rc" -eq 0 ] && [ "$(steps "$FAKE_DIR/out" 'done')" -eq 6 ] \
  && grep -q '^agent add: \[7/7\] start: not yet' "$FAKE_DIR/out" && [ "$next" -eq 1 ] \
  && tail -n 1 "$FAKE_DIR/out" | grep -qF "next step: ask the owner to approve dev1@host-b (owner: crewboard approve dev1@host-b --initiative demo;" \
  && tail -n 1 "$FAKE_DIR/out" | grep -qF -- "then run: crewboard agent add demo-owner/demo-repo --agent-dir '$bd'" \
  && [ "$(cat "$FAKE_DIR/calls")" = "--config-dir $bd join --name dev1@host-b --runtime claude --repo demo-owner/demo-repo" ] \
  && [ "$(cut -d'|' -f4 "$FAKE_DIR/ctx")" = "boot-token-SECRET" ] \
  && [ "$(git -C "$wt" config user.email)" = "4242+crew-bot@users.noreply.github.com" ] \
  && [ "$(git -C "$wt" config user.name)" = "crew-bot" ] \
  && [ "$(git -C "$wt" config --get-all credential.helper | tail -n 1)" = "!$bd/bin/gh auth git-credential" ] \
  && [ "$(git -C "$wt" remote get-url origin)" = "https://github.com/demo-owner/demo-repo" ] \
  && [ -x "$bd/bin/gh" ] && [ "$(readlink "$bd/agent.env")" = "$BOOT/crew.env" ] && [ ! -s "$FAKE_DIR/err" ] && ok=0
case_result "B1 bootstrap: 6 steps done, join posted, one next step" $ok "rc=$rc, done=$(steps "$FAKE_DIR/out" 'done'), calls=$(ncalls), next=$next"
# B1b: the token is never printed, not even into the log of the steps
leak=0; grep -q 'boot-token-SECRET' "$FAKE_DIR/out" "$FAKE_DIR/err" "$bd/bootstrap.log" && leak=1
case_result "B1b bootstrap prints no token (output and log)" $leak "leak=$([ $leak -eq 0 ] && echo no || echo YES)"

# B2: a rerun before the approval: every step already done, no second join, start runs in the
# work dir with the agent environment (shim first, config dir, no token variable), refused as
# not approved -> the same one next step, exit 0
new_case b2
bs "$BD" "$BOOT/work" >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(steps "$FAKE_DIR/out" 'already done')" -eq 6 ] && [ "$(steps "$FAKE_DIR/out" 'done ')" -eq 0 ] \
  && [ "$(cat "$FAKE_DIR/calls")" = "start" ] && [ "$(cat "$FAKE_DIR/ctx")" = "$wt|$bd|$bd/bin|unset" ] \
  && grep -q '^agent add: \[7/7\] start: not yet' "$FAKE_DIR/out" \
  && [ "$(grep -c '^agent add: next step: ask the owner to approve dev1@host-b' "$FAKE_DIR/out")" -eq 1 ] && ok=0
case_result "B2 rerun: 6x already done, start as the agent, still waiting" $ok "rc=$rc, already=$(steps "$FAKE_DIR/out" 'already done'), calls=$(cat "$FAKE_DIR/calls"), ctx=$(cat "$FAKE_DIR/ctx")"

# B3: the rerun after the approval: start passes; the next step starts the runtime with --exec
new_case b3
FAKE_START=ok bs "$BD" "$BOOT/work" >"$FAKE_DIR/out" 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^agent add: \[7/7\] start: done' "$FAKE_DIR/out" \
  && grep -qx 'agent add: next step: start the runtime in the work dir:' "$FAKE_DIR/out" \
  && grep -A1 -x 'agent add: next step: start the runtime in the work dir:' "$FAKE_DIR/out" | tail -n 1 | grep -q "^cd '$wt' && crewboard agent exec --agent-dir '$bd' -- claude '" \
  && tail -n 1 "$FAKE_DIR/out" | grep -qF "agent add: optional: an unattended agent: crewboard wake install --scheduler " \
  && tail -n 1 "$FAKE_DIR/out" | grep -qF -- "--runtime claude --agent-dir '$bd' --initiative demo --env-file '$BOOT/crew.env' --work-dir '$wt' --shim-dir '$bd/bin'" \
  && [ "$(grep -c '^agent add: next step:' "$FAKE_DIR/out")" -eq 1 ] && ok=0
case_result "B3 rerun after approval: start done, next step runs the runtime" $ok "rc=$rc, last=$(tail -n 1 "$FAKE_DIR/out" | cut -c1-60)..."

# B4: start refused for another reason: the step fails, names doctor --setup as the fix, and
# exits with the code of start
new_case b4
FAKE_START=other bs "$BD" "$BOOT/work" >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
ok=1; [ "$rc" -eq 2 ] && grep -q '^agent add: \[7/7\] start: FAIL: crewboard start exit 2' "$FAKE_DIR/err" \
  && grep -q 'agent-gh-not-crew' "$FAKE_DIR/err" && grep -qF -- "fix: see which setup check fails: cd '$wt' && crewboard agent exec --agent-dir '$bd' -- crewboard doctor --setup" "$FAKE_DIR/err" \
  && ! grep -q 'next step' "$FAKE_DIR/out" && ok=0
case_result "B4 start refused: step named, doctor --setup as fix, rc 2" $ok "rc=$rc"

# B5: a red proof for each other step. The env file is open to others: step 1 fails, nothing runs
new_case b5
printf 'GH_TOKEN=x\n' >"$BOOT/open.env"; chmod 644 "$BOOT/open.env"
PATH="$BOOT/realgh:$PATH" "$BOOT/crewboard" agent add --agent-dir "$BOOT/agents/open" --env-file "$BOOT/open.env" \
  --agent dev2@host-b --runtime codex --repo demo-owner/demo-repo --work-dir "$BOOT/work2" >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
ok=1; [ "$rc" -eq 4 ] && grep -q '^agent add: \[1/7\] agent dir: FAIL' "$FAKE_DIR/err" \
  && grep -qF "fix: chmod 600 $BOOT/open.env" "$FAKE_DIR/err" && [ ! -e "$BOOT/agents/open" ] && [ "$(ncalls)" -eq 0 ] && ok=0
case_result "B5 env file mode 644: step 1 FAIL with chmod fix, nothing runs" $ok "rc=$rc, calls=$(ncalls)"
# B5b: join fails: step 2 names the fix, the exit code of join comes back, no shim is made
new_case b5b
FAKE_JOIN_RC=3 bs "$BOOT/agents/dev3" "$BOOT/work3" >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
ok=1; [ "$rc" -eq 3 ] && grep -q '^agent add: \[2/7\] join: FAIL: crewboard join exit 3' "$FAKE_DIR/err" \
  && grep -q 'fake join failed' "$FAKE_DIR/err" && grep -q 'fix: .*--board-file' "$FAKE_DIR/err" \
  && [ ! -e "$BOOT/agents/dev3/bin/gh" ] && [ ! -e "$BOOT/work3" ] && ok=0
case_result "B5b join fails: step 2 FAIL, rc 3, no shim, no clone" $ok "rc=$rc"
# B5c: the crew id lookup gives no number (wrong --crew-login): step 4 fails
new_case b5c
bs "$BD" "$BOOT/work" --crew-login nobody >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
ok=1; [ "$rc" -eq 4 ] && grep -q '^agent add: \[4/7\] crew id: FAIL: no numeric id for nobody' "$FAKE_DIR/err" \
  && [ "$(git -C "$wt" config user.email)" = "4242+crew-bot@users.noreply.github.com" ] && ok=0
case_result "B5c wrong crew login: step 4 FAIL, git identity untouched" $ok "rc=$rc"
# B5d: the work dir exists, is not empty and is no checkout: step 5 fails, nothing is cloned there
new_case b5d
mkdir -p "$BOOT/notgit"; : >"$BOOT/notgit/keep"
bs "$BD" "$BOOT/notgit" >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
ok=1; [ "$rc" -eq 4 ] && grep -q '^agent add: \[5/7\] work dir: FAIL: .* is not empty and is not a git checkout' "$FAKE_DIR/err" \
  && [ ! -e "$BOOT/notgit/.git" ] && ok=0
case_result "B5d non-empty non-git work dir: step 5 FAIL" $ok "rc=$rc"
# B5e: the work dir is a checkout of another repo: step 5 fails too
new_case b5e
"${GIT[@]}" -C "$BOOT" init -q other 2>/dev/null; git -C "$BOOT/other" remote add origin https://github.com/someone/else
bs "$BD" "$BOOT/other" >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
ok=1; [ "$rc" -eq 4 ] && grep -q '^agent add: \[5/7\] work dir: FAIL: .* is a checkout of .*someone/else, not of demo-owner/demo-repo' "$FAKE_DIR/err" && ok=0
case_result "B5e checkout of another repo: step 5 FAIL" $ok "rc=$rc"
# B5f: a wrong git identity in the work dir is fixed by step 6 (not "already done")
new_case b5f
git -C "$wt" config user.email someone@example.org
bs "$BD" "$BOOT/work" >"$FAKE_DIR/out" 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^agent add: \[6/7\] git config: done' "$FAKE_DIR/out" \
  && [ "$(git -C "$wt" config user.email)" = "4242+crew-bot@users.noreply.github.com" ] && ok=0
case_result "B5f a changed git e-mail: step 6 sets it again" $ok "rc=$rc"
# B6: usage errors (a missing target, a bad option value) exit 64 and run nothing
new_case b6
crewboard agent add --agent-dir "$BD" --env-file "$BOOT/crew.env" --agent dev1@host-b --runtime claude --work-dir "$wt" >/dev/null 2>"$FAKE_DIR/e1"; r1=$?
crewboard agent add --agent-dir "$BD" --env-file "$BOOT/crew.env" --agent dev1@host-b --runtime vim --repo a/b --work-dir "$wt" >/dev/null 2>"$FAKE_DIR/e2"; r2=$?
ok=1; [ "$r1" -eq 64 ] && [ "$r2" -eq 64 ] && grep -q 'a repository <owner>/<repo> or a board file is required' "$FAKE_DIR/e1" \
  && grep -q "invalid value 'vim'" "$FAKE_DIR/e2" && [ "$(ncalls)" -eq 0 ] && ok=0
case_result "B6 bootstrap usage errors -> 64, nothing runs" $ok "rc=$r1,$r2"

# B7: --role coordinator: the same steps as a dev; the approve line names --role coordinator;
# after the approval the next step names the headless bootstrap of the same coordinator
new_case b7
CD="$BOOT/agents/coord"
bs "$CD" "$BOOT/workc" --role coordinator >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
cd_abs=$(cd "$CD" && pwd)
ok=1; [ "$rc" -eq 0 ] && [ "$(steps "$FAKE_DIR/out" 'done')" -eq 6 ] \
  && tail -n 1 "$FAKE_DIR/out" | grep -qF "next step: ask the owner to approve dev1@host-b (owner: crewboard approve dev1@host-b --initiative demo --role coordinator)" \
  && [ ! -s "$FAKE_DIR/err" ] && ok=0
case_result "B7 coordinator bootstrap: 6 steps, approve line has --role coordinator" $ok "rc=$rc, done=$(steps "$FAKE_DIR/out" 'done')"
new_case b7b
FAKE_START=ok bs "$CD" "$BOOT/workc" --role coordinator >"$FAKE_DIR/out" 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(steps "$FAKE_DIR/out" 'already done')" -eq 6 ] && grep -q '^agent add: \[7/7\] start: done' "$FAKE_DIR/out" \
  && tail -n 1 "$FAKE_DIR/out" | grep -qF -- "agent add: optional: the headless tick of this coordinator: crewboard agent add --role headless-coordinator --from '$cd_abs' --agent-dir '$cd_abs-headless' --env-file '$BOOT/crew.env'" \
  && [ "$(grep -c '^agent add: next step:' "$FAKE_DIR/out")" -eq 1 ] && ok=0
case_result "B7b coordinator rerun after approval: next step is the headless one" $ok "rc=$rc"

# B8: --role headless-coordinator: the headless dir is made from the interactive one (toml with
# headless = true first, key mode 600), nothing is printed of the key; a rerun is all "already
# done"; the last line is the crewboard wake install command
new_case b8
mkdir -p "$cd_abs/keys"; printf 'PRIVATE-KEY-SECRET\n' >"$cd_abs/keys/dev1@host-b"; chmod 600 "$cd_abs/keys/dev1@host-b"
HD="$BOOT/agents/coord-headless"
hs() { "$BOOT/crewboard" agent add --agent-dir "$HD" --role headless-coordinator --from "$CD" --env-file "$BOOT/crew.env" "$@"; }
hs >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
hd=$(cd "$HD" && pwd)
mode() { stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"; }
ok=1; [ "$rc" -eq 0 ] && [ "$(grep -c '^agent add: \[[1-3]/3\] [a-z ]*: done' "$FAKE_DIR/out")" -eq 3 ] \
  && [ "$(head -n 1 "$HD/demo.toml")" = "headless = true" ] && [ "$(sed -n 2p "$HD/demo.toml")" = 'name = "dev1@host-b"' ] \
  && [ "$(mode "$HD/demo.toml")" = 600 ] && [ "$(mode "$HD/keys")" = 700 ] && [ "$(mode "$HD/keys/dev1@host-b")" = 600 ] \
  && cmp -s "$cd_abs/keys/dev1@host-b" "$HD/keys/dev1@host-b" && [ "$(readlink "$HD/agent.env")" = "$BOOT/crew.env" ] \
  && ! grep -q 'headless' "$cd_abs/demo.toml" \
  && [ "$(tail -n 1 "$FAKE_DIR/out" | cut -c1-38)" = "agent add: next step: install the tick" ] \
  && tail -n 1 "$FAKE_DIR/out" | grep -qF -- "crewboard wake install --scheduler " \
  && tail -n 1 "$FAKE_DIR/out" | grep -qF -- "--tick --agent-dir '$hd' --initiative demo --env-file '$BOOT/crew.env'" && ok=0
case_result "B8 headless coordinator: dir made, toml headless first, key 600" $ok "rc=$rc"
leak=0; grep -q 'PRIVATE-KEY-SECRET\|boot-token-SECRET' "$FAKE_DIR/out" "$FAKE_DIR/err" && leak=1
case_result "B8b headless bootstrap prints no key or token" $leak "leak=$([ $leak -eq 0 ] && echo no || echo YES)"
new_case b8c
hs >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(grep -c '^agent add: \[[1-3]/3\] [a-z ]*: already done' "$FAKE_DIR/out")" -eq 3 ] \
  && [ "$(grep -c '^headless' "$HD/demo.toml")" -eq 1 ] && [ "$(grep -c '^agent add: next step:' "$FAKE_DIR/out")" -eq 1 ] && ok=0
case_result "B8c headless rerun: 3x already done, one headless line" $ok "rc=$rc"

# B9: refusals: --from is itself headless; no key; the same dir; a dev option; each exits and
# leaves no half-made headless dir behind
new_case b9
hs2() { "$BOOT/crewboard" agent add --agent-dir "$1" --role headless-coordinator --from "$2" --env-file "$BOOT/crew.env"; }
hs2 "$BOOT/agents/h2" "$HD" >/dev/null 2>"$FAKE_DIR/e1"; r1=$?
mkdir -p "$BOOT/agents/nokey"; cp "$CD/demo.toml" "$BOOT/agents/nokey/demo.toml"
hs2 "$BOOT/agents/h3" "$BOOT/agents/nokey" >/dev/null 2>"$FAKE_DIR/e2"; r2=$?
hs2 "$CD" "$CD" >/dev/null 2>"$FAKE_DIR/e3"; r3=$?
"$BOOT/crewboard" agent add --agent-dir "$BOOT/agents/h4" --role headless-coordinator --from "$CD" --env-file "$BOOT/crew.env" --agent x >/dev/null 2>"$FAKE_DIR/e4"; r4=$?
ok=1; [ "$r1" -eq 4 ] && [ "$r2" -eq 4 ] && [ "$r3" -eq 4 ] && [ "$r4" -eq 4 ] \
  && grep -q 'is already headless' "$FAKE_DIR/e1" && grep -q 'no key file' "$FAKE_DIR/e2" \
  && grep -q 'is the interactive dir' "$FAKE_DIR/e3" && grep -q 'takes only --from, --agent-dir and --env-file' "$FAKE_DIR/e4" \
  && [ ! -e "$BOOT/agents/h2" ] && [ ! -e "$BOOT/agents/h3" ] && [ ! -e "$BOOT/agents/h4" ] && ok=0
case_result "B9 headless refusals: headless source, no key, same dir, usage" $ok "rc=$r1,$r2,$r3,$r4"

# ---------------- PATH of a scheduler ----------------
# cron and launchd start the wake with a minimal PATH. `cargo install` puts crewboard in
# ~/.cargo/bin. The scripts add ~/.cargo/bin and ~/.local/bin at the END of the PATH when the
# directory exists and is not on the PATH yet. Each case runs with `env -i`, its own HOME and
# PATH=/usr/bin:/bin, so neither the fake of $T/bin nor a real crewboard of this host is found.
echo "== PATH of a scheduler"
fake_in() { # dir: a fake crewboard that records PATH and args; exit $FAKE_RC (default 1)
  mkdir -p "$1"
  cat >"$1/crewboard" <<'STUB'
#!/bin/bash
[ "$1 $2" = "wake --help" ] && exit 0   # the probe of the shim
printf '%s\n' "$PATH" >"$FAKE_DIR/path.seen"
printf '%s\n' "$*" >>"$FAKE_DIR/calls"
exit "${FAKE_RC:-1}"
STUB
  chmod +x "$1/crewboard"
}
MIN=/usr/bin:/bin
seen_path() { cat "$FAKE_DIR/path.seen" 2>/dev/null; }

# H1: crewboard only in $HOME/.cargo/bin -> poll-and-wake finds it; the missing ~/.local/bin is not added
new_case h1; H1="$T/home-h1"; fake_in "$H1/.cargo/bin"
env -i HOME="$H1" PATH="$MIN" FAKE_DIR="$FAKE_DIR" FAKE_RC=0 /bin/bash "$P" --wake-cmd 'true' \
  --lock-dir "$FAKE_DIR/lock" --work-backoff off >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(cat "$FAKE_DIR/calls")" = "wake --wake-cmd true --lock-dir $FAKE_DIR/lock --work-backoff off" ] && [ "$(seen_path)" = "$MIN:$H1/.cargo/bin" ] && ok=0
case_result "H1 poll-and-wake: minimal PATH finds ~/.cargo/bin/crewboard" $ok "rc=$rc, calls=$(ncalls), path=$(seen_path)"

# H2: crewboard only in $HOME/.local/bin, no ~/.cargo/bin -> found; the missing dir is not added
new_case h2; H2="$T/home-h2"; fake_in "$H2/.local/bin"
env -i HOME="$H2" PATH="$MIN" FAKE_DIR="$FAKE_DIR" FAKE_RC=0 /bin/bash "$P" --wake-cmd 'true' \
  --lock-dir "$FAKE_DIR/lock" --work-backoff off >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(seen_path)" = "$MIN:$H2/.local/bin" ] && ok=0
case_result "H2 poll-and-wake: ~/.local/bin added, missing ~/.cargo/bin not" $ok "rc=$rc, path=$(seen_path)"

# H3: neither dir exists -> nothing is added; the missing crewboard is an error (exit 127, the shell says so), not a quiet board
new_case h3; H3="$T/home-h3"; mkdir -p "$H3"
env -i HOME="$H3" PATH="$MIN" FAKE_DIR="$FAKE_DIR" /bin/bash "$P" --wake-cmd 'true' \
  --lock-dir "$FAKE_DIR/lock" --work-backoff off >/dev/null 2>"$FAKE_DIR/err"; rc=$?
ok=1; [ "$rc" -eq 127 ] && [ "$(ncalls)" -eq 0 ] && grep -q '^poll-and-wake: crewboard not found on PATH (.*); install it or set CREWBOARD_BIN$' "$FAKE_DIR/err" && ok=0
case_result "H3 no install dir: rc 127 and said, not silent" $ok "rc=$rc, err=$(cat "$FAKE_DIR/err")"

# H4: the dirs are on the PATH already -> the caller's PATH stays as it is (no copy, same order)
new_case h4; H4="$T/home-h4"; fake_in "$H4/.cargo/bin"; mkdir -p "$H4/.local/bin"
env -i HOME="$H4" PATH="$H4/.cargo/bin:$MIN:$H4/.local/bin" FAKE_DIR="$FAKE_DIR" FAKE_RC=0 /bin/bash "$P" --wake-cmd 'true' \
  --lock-dir "$FAKE_DIR/lock" --work-backoff off >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(seen_path)" = "$H4/.cargo/bin:$MIN:$H4/.local/bin" ] && ok=0
case_result "H4 dirs already on PATH -> PATH unchanged" $ok "rc=$rc, path=$(seen_path)"

# H5: agent-wake.sh with a minimal PATH finds crewboard in ~/.cargo/bin (the wake itself is `crewboard wake`:
# the fake sees the options of the dev agent and the shim dir)
new_case h5; H5="$T/home-h5"; fake_in "$H5/.cargo/bin"
env -i HOME="$H5" PATH="$MIN" FAKE_DIR="$FAKE_DIR" FAKE_RC=0 /bin/bash "$A" --runtime claude "${AWC[@]}" \
  --shim-dir "$AW/agent/bin" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
last=$(tail -n 1 "$FAKE_DIR/calls" 2>/dev/null)
ok=1; [ "$rc" -eq 0 ] && case "$last" in "wake --agent dev@host-a --initiative demo "*"--runtime claude --work-dir $AW/wd --shim-dir $AW/agent/bin"*) true ;; *) false ;; esac \
  && [ "$(seen_path)" = "$MIN:$H5/.cargo/bin" ] && ok=0
case_result "H5 agent-wake: minimal PATH finds crewboard; it gets the dev agent options" $ok "rc=$rc, last=${last:0:140}"

# ---------------- per-agent state (per agent, not per initiative) ----------------
# Without --lock-dir and --state-dir the state lives per AGENT: crewboard/agents/<initiative>-<config dir name>.
echo "== per-agent state"
ST="$XDG_STATE_HOME/crewboard"
# S1: two agents of one initiative on one host: the second wakes while the first one runs
new_case s1 '0||' '0||'
SA="$T/s1"; mkdir -p "$SA/agents/dev1" "$SA/agents/dev2"
SLOW='echo "$CREWBOARD_CONFIG_DIR" >> "$FAKE_DIR/wakes"; sleep 3'
"$P" --initiative s1 --config-dir "$SA/agents/dev1" --wake-cmd "$SLOW" --work-backoff off --heartbeat-every 0 >/dev/null 2>&1 &
first=$!
i=0; while [ ! -s "$FAKE_DIR/wakes" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done   # the first wake runs
"$P" --initiative s1 --config-dir "$SA/agents/dev2" --wake-cmd "$SLOW" --work-backoff off --heartbeat-every 0 >/dev/null 2>&1; rc2=$?
wait $first; rc1=$?
ok=1; [ "$rc1" -eq 0 ] && [ "$rc2" -eq 0 ] && [ "$(WAKES)" -eq 2 ] \
  && grep -qx "$SA/agents/dev1" "$FAKE_DIR/wakes" && grep -qx "$SA/agents/dev2" "$FAKE_DIR/wakes" \
  && grep -q 'wake finished OK' "$ST/agents/s1-dev1/wake.log" && grep -q 'wake finished OK' "$ST/agents/s1-dev2/wake.log" \
  && ! grep -q 'already runs' "$ST/agents/s1-dev1/wake.log" "$ST/agents/s1-dev2/wake.log" && ok=0
case_result "S1 two agents, one host, one initiative: both wake" $ok "wakes=$(WAKES), rc=$rc1/$rc2"
# S1b: the other direction: the SAME agent twice -> the second run starts no second wake
new_case s1b '0||' '0||'
"$P" --initiative s1b --config-dir "$SA/agents/dev1" --wake-cmd "$SLOW" --work-backoff off --heartbeat-every 0 >/dev/null 2>&1 &
first=$!
i=0; while [ ! -s "$FAKE_DIR/wakes" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done   # the first wake runs
"$P" --initiative s1b --config-dir "$SA/agents/dev1" --wake-cmd "$SLOW" --work-backoff off --heartbeat-every 0 >/dev/null 2>&1; rc2=$?
wait $first
ok=1; [ "$rc2" -eq 0 ] && [ "$(WAKES)" -eq 1 ] && grep -q 'already runs' "$ST/agents/s1b-dev1/wake.log" && ok=0
case_result "S1b the same agent twice: one wake, the second waits" $ok "wakes=$(WAKES), rc=$rc2"
# S2: --print-state-dir: per config dir, --agent without a config dir, --lock-dir, and the old default
d1=$("$P" --wake-cmd x --initiative demo --config-dir /x/agents/dev1 --print-state-dir)
d2=$("$P" --wake-cmd x --initiative demo --config-dir /x/agents/dev2 --print-state-dir)
d3=$("$P" --wake-cmd x --initiative demo --agent 'dev@host-a' --print-state-dir)
d4=$("$P" --wake-cmd x --initiative demo --config-dir /x/agents/dev1 --lock-dir /y/wake.lock --print-state-dir)
d5=$("$P" --wake-cmd x --initiative demo --print-state-dir)
ok=1; [ "$d1" = "$ST/agents/demo-dev1" ] && [ "$d2" = "$ST/agents/demo-dev2" ] && [ "$d3" = "$ST/agents/demo-dev_host-a" ] \
  && [ "$d4" = /y ] && [ "$d5" = "$ST/demo" ] && ok=0
case_result "S2 state dir per agent; --lock-dir and no agent as before" $ok "$d1 | $d3 | $d4 | $d5"
# S3: migration: the old per-initiative last-wake is read once (so the back-off holds), and a live
# old per-initiative lock (an older copy of the script still runs) starts no new wake
new_case s3 '1||'; echo 0 >"$FAKE_DIR/plan.mine"
mkdir -p "$ST/s3"; echo $(( $(date +%s) - 60 )) >"$ST/s3/last-wake"
"$P" --initiative s3 --config-dir "$SA/agents/dev1" --wake-cmd "$WAKE" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(WAKES)" -eq 0 ] && [ "$(nlines "$FAKE_DIR/calls.mine")" -eq 0 ] \
  && [ "$(cat "$ST/agents/s3-dev1/last-wake")" = "$(cat "$ST/s3/last-wake")" ] && ok=0
case_result "S3 old last-wake is read once: the back-off holds" $ok "wakes=$(WAKES), mine calls=$(nlines "$FAKE_DIR/calls.mine")"
new_case s3b '0||'
sleep 30 & live=$!
mkdir -p "$ST/s3b/wake.lock"; echo "$live" >"$ST/s3b/wake.lock/pid"
"$P" --initiative s3b --config-dir "$SA/agents/dev1" --wake-cmd "$WAKE" >/dev/null 2>&1; rc=$?
calls_live=$(ncalls)
kill $live 2>/dev/null; wait $live 2>/dev/null
"$P" --initiative s3b --config-dir "$SA/agents/dev1" --wake-cmd "$WAKE" --heartbeat-every 0 >/dev/null 2>&1; rc2=$?
ok=1; [ "$rc" -eq 0 ] && [ "$calls_live" -eq 0 ] && grep -q 'an older per-initiative wake runs' "$ST/agents/s3b-dev1/wake.log" \
  && [ "$rc2" -eq 0 ] && [ "$(WAKES)" -eq 1 ] && ok=0
case_result "S3b live old per-initiative lock: no wake; a dead one is ignored" $ok "calls while live=$calls_live, wakes after=$(WAKES)"
# S3c: the live old lock belongs to ANOTHER agent (its holder runs with another --config-dir):
# this agent wakes; the same --config-dir (also with a final /) still waits
new_case s3c '0||' '0||'
bash -c 'sleep 30; :' holder --config-dir "$SA/agents/other" & other=$!
mkdir -p "$ST/s3c/wake.lock"; echo "$other" >"$ST/s3c/wake.lock/pid"
"$P" --initiative s3c --config-dir "$SA/agents/dev1" --wake-cmd "$WAKE" --heartbeat-every 0 >/dev/null 2>&1; rc=$?
w_other=$(WAKES)
kill $other 2>/dev/null; wait $other 2>/dev/null
bash -c 'sleep 30; :' holder --config-dir "$SA/agents/dev1/" & same=$!
echo "$same" >"$ST/s3c/wake.lock/pid"
"$P" --initiative s3c --config-dir "$SA/agents/dev1" --wake-cmd "$WAKE" --heartbeat-every 0 >/dev/null 2>&1; rc2=$?
w_same=$(WAKES)
kill $same 2>/dev/null; wait $same 2>/dev/null
ok=1; [ "$rc" -eq 0 ] && [ "$w_other" -eq 1 ] && [ "$rc2" -eq 0 ] && [ "$w_same" -eq 1 ] && ok=0
case_result "S3c old lock of another agent: this agent wakes; the same config dir waits" $ok "wakes after other=$w_other, after same=$w_same"

# ---------------- crewboard exit 3 and exit 4 in the wrapper (move 14) ----------------
echo "== exit 3 and exit 4"
# R1: exit 3 is retried; the retry finds events and wakes once
new_case r1 '3||HTTP 502' '0||'
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --retry-delays 0 --heartbeat-every 0 >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(WAKES)" -eq 1 ] && [ "$(ncalls)" -eq 2 ] && grep -q 'crewboard poll: exit 3 (try 1): HTTP 502; retry in 0 s' "$FAKE_DIR/wake.log" && ok=0
case_result "R1 exit 3, then events: retried, one wake" $ok "rc=$rc, polls=$(ncalls), wakes=$(WAKES)"
# R2: exit 3 in every try: rc 3, no wake; the 2nd failed run starts a pause, and the next run calls nothing
new_case r2 '3||down' '3||down' '3||down' '3||down' '1||'
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --retry-delays 0 >/dev/null 2>&1; rc1=$?
read -r c1 u1 <"$FAKE_DIR/net-backoff"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --retry-delays 0 >/dev/null 2>&1; rc2=$?
read -r c2 u2 <"$FAKE_DIR/net-backoff"
before=$(ncalls)
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --retry-delays 0 >/dev/null 2>&1; rc3=$?
paused=$(( $(ncalls) - before ))
now=$(date +%s)
ok=1; [ "$rc1" -eq 3 ] && [ "$rc2" -eq 3 ] && [ "$rc3" -eq 0 ] && [ "$(WAKES)" -eq 0 ] && [ "$c1" = 1 ] && [ "$c2" = 2 ] \
  && [ "$u1" -le "$now" ] && [ "$u2" -gt $((now + 200)) ] && [ "$paused" -eq 0 ] && ok=0
case_result "R2 exit 3 every try: rc 3; 2nd failed run pauses the next run" $ok "rc=$rc1/$rc2/$rc3, count=$c1/$c2, calls in pause=$paused"
# R2b: when the pause is over, a good poll clears the back-off
echo "2 $(( $(date +%s) - 1 ))" >"$FAKE_DIR/net-backoff"
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --retry-delays 0 --work-backoff off >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ ! -f "$FAKE_DIR/net-backoff" ] && ok=0
case_result "R2b pause over, a good poll clears the back-off" $ok "rc=$rc, file left=$([ -f "$FAKE_DIR/net-backoff" ] && echo yes || echo no)"
# X1: exit 4: rc 4, one STOPPED line, a say to the coordinator; the next run calls nothing; a newer
# env file lifts the stop
new_case x1 '4||configuration error: key file missing' '0||'
echo '0|demo/app#3                         In Progress      P1  order 1' >"$FAKE_DIR/plan.mine"
printf 'GH_TOKEN=x\n' >"$FAKE_DIR/x.env"; chmod 600 "$FAKE_DIR/x.env"
"$P" --env-file "$FAKE_DIR/x.env" --agent dev@host-a --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>"$FAKE_DIR/err"; rc1=$?
c1=$(ncalls)
"$P" --env-file "$FAKE_DIR/x.env" --agent dev@host-a --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc2=$?
c2=$(ncalls); m2=$(nlines "$FAKE_DIR/calls.mine")
ok=1; [ "$rc1" -eq 4 ] && [ "$rc2" -eq 0 ] && [ "$c1" -eq 2 ] && [ "$c2" -eq 2 ] && [ "$m2" -eq 1 ] && [ -f "$FAKE_DIR/stopped" ] \
  && [ "$(grep -c 'STOPPED: crewboard poll exit 4: configuration error: key file missing' "$FAKE_DIR/wake.log")" -eq 1 ] \
  && grep -q 'STOPPED' "$FAKE_DIR/err" && grep -q '^say demo/app#3 --to coordinator --kind info --override-unread --body The wake wrapper of dev@host-a stopped' "$FAKE_DIR/calls" \
  && [ "$(WAKES)" -eq 0 ] && ok=0
case_result "X1 exit 4: stop, one line, say; next run calls nothing" $ok "rc=$rc1/$rc2, calls=$c1/$c2"
touch -t 202001010000 "$FAKE_DIR/stopped"
printf '1||\n' >>"$FAKE_DIR/plan"
"$P" --env-file "$FAKE_DIR/x.env" --agent dev@host-a --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --work-backoff off >/dev/null 2>&1; rc3=$?
ok=1; [ "$rc3" -eq 0 ] && [ "$(ncalls)" -eq 3 ] && [ ! -f "$FAKE_DIR/stopped" ] && grep -q "stop lifted: $FAKE_DIR/x.env changed" "$FAKE_DIR/wake.log" && ok=0
case_result "X1b a newer env file lifts the stop: polls again" $ok "rc=$rc3, calls=$(ncalls)"
# X2: the other direction: exit 2 is no stop; the next run polls again
new_case x2 '2||refused [rule=x]' '1||'
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --work-backoff off >/dev/null 2>&1; rc1=$?
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --work-backoff off >/dev/null 2>&1; rc2=$?
ok=1; [ "$rc1" -eq 2 ] && [ "$rc2" -eq 0 ] && [ "$(ncalls)" -eq 2 ] && [ ! -f "$FAKE_DIR/stopped" ] && ok=0
case_result "X2 exit 2 is no stop: the next run polls again" $ok "rc=$rc1/$rc2, calls=$(ncalls)"

# ---------------- time budget of a wake (move 6) ----------------
echo "== time budget"
# host-lock.sh would wait for a real CI runner of this host; the cases see no CI.
export HOST_LOCK_PS_CMD=true
TBW="$T/tb-work"; mkdir -p "$TBW"
git -c init.defaultBranch=main -C "$TBW" init -q 2>/dev/null
git -C "$TBW" -c user.email=t@x -c user.name=t commit -q --allow-empty -m one
git -C "$TBW" checkout -q -b agent-branch
TBHEAD=$(git -C "$TBW" rev-parse HEAD)
# TB1: a wake over its budget: the wrapper stops it with its children, says on the ticket (branch,
# head, "stopped at the time budget"), releases the host lock of the agent, exits 0
new_case tb1 '0||' '0||'
echo '0|demo/app#12                        In Progress      P1  order 1' >"$FAKE_DIR/plan.mine"
HL="$FAKE_DIR/heavy.lock.d"
bash "$DRV/host-lock.sh" take "$HL" --holder dev@host-a --no-board --pid 0 --expect 600 >/dev/null 2>&1; rt=$?
held_before=$([ -d "$HL" ] && [ "$rt" -eq 0 ] && echo yes || echo no)
LONG='sleep 300 & echo $! > "$FAKE_DIR/child"; echo wake >> "$FAKE_DIR/wakes"; wait'
t0=$(date +%s)
CREWBOARD_KILL_GRACE=2 "$P" --agent dev@host-a --wake-cmd "$LONG" --lock-dir "$FAKE_DIR/lock" --max-wake 2s \
  --work-dir "$TBW" --host-lock "$HL" --heartbeat-every 0 --work-backoff off >/dev/null 2>&1; rc=$?
dur=$(( $(date +%s) - t0 ))
child=$(cat "$FAKE_DIR/child" 2>/dev/null)
sayline=$(grep '^say ' "$FAKE_DIR/calls")
alive=no; [ -n "$child" ] && kill -0 "$child" 2>/dev/null && alive=yes
ok=1; [ "$rc" -eq 0 ] && [ "$held_before" = yes ] && [ "$dur" -lt 15 ] && [ "$alive" = no ] && [ ! -d "$HL" ] && [ ! -d "$FAKE_DIR/lock" ] \
  && [ "$sayline" = "say demo/app#12 --to coordinator --kind info --override-unread --body stopped at the time budget (2s) of the wake wrapper. Ticket demo/app#12, no branch of the ticket and no PR yet (the work dir is on agent-branch at $TBHEAD). The next wake continues from here." ] \
  && grep -q 'wake stopped at the time budget of 2s' "$FAKE_DIR/wake.log" && grep -q "host lock $HL: released for dev@host-a" "$FAKE_DIR/wake.log" && ok=0
[ -n "$child" ] && kill "$child" 2>/dev/null
# a slow run says where the time went: the timestamped log of the wrapper and the CI processes of this host
slow=""; [ "$dur" -ge 15 ] && slow=" | wake.log: $(tr '\n' '|' <"$FAKE_DIR/wake.log" 2>/dev/null | cut -c1-700) | Runner.Worker processes: $(ps -axo command= | grep -c '[R]unner.Worker'), load: $(uptime | sed 's/.*load averages*: //')"
case_result "TB1 over budget: stopped with its child, say, host lock released" $ok "rc=$rc, ${dur}s, child alive=$alive, lock left=$([ -d "$HL" ] && echo yes || echo no)$slow"
# TB2: the other direction: a wake inside its budget ends by itself: no say, the host lock stays
new_case tb2 '0||'
echo '0|demo/app#12                        In Progress      P1  order 1' >"$FAKE_DIR/plan.mine"
HL="$FAKE_DIR/heavy.lock.d"
bash "$DRV/host-lock.sh" take "$HL" --holder dev@host-a --no-board --pid 0 --expect 600 >/dev/null 2>&1
"$P" --agent dev@host-a --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --max-wake 20s \
  --work-dir "$TBW" --host-lock "$HL" --heartbeat-every 0 --work-backoff off >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(WAKES)" -eq 1 ] && ! grep -q '^say ' "$FAKE_DIR/calls" && [ -d "$HL" ] && grep -q 'wake finished OK' "$FAKE_DIR/wake.log" && ok=0
case_result "TB2 inside the budget: no say, host lock stays" $ok "rc=$rc, says=$(grep -c '^say ' "$FAKE_DIR/calls")"
bash "$DRV/host-lock.sh" release "$HL" --holder dev@host-a >/dev/null 2>&1
# TB10: the stop of a wake that holds a `crewboard lock run` lock leaves no lock dir,
# not even an empty one: the release renames the dir away before it removes it, and the run
# forwards the TERM of the stop to its job and releases. (The old proof put a stub `rm` on the PATH
# that killed the script at each `rm`; the Rust code calls no `rm`. Its twin is the Rust test
# lock::run::dt5_a_release_renames_before_it_removes_so_no_empty_dir_is_ever_seen.)
new_case tb10 '0||' '0||'
echo '0|demo/app#12                        In Progress      P1  order 1' >"$FAKE_DIR/plan.mine"
HL="$FAKE_DIR/heavy.lock.d"
printf '%s\n' 'sleep 300 & wait' >"$FAKE_DIR/slow.sh"
printf '%s\n' "bash '$DRV/host-lock.sh' run '$HL' --holder dev@host-a --no-board -- bash '$FAKE_DIR/slow.sh' &" 'wait' >"$FAKE_DIR/wake10.sh"
WAKE4="sh $FAKE_DIR/wake10.sh"
CREWBOARD_KILL_GRACE=6 "$P" --agent dev@host-a --wake-cmd "$WAKE4" --lock-dir "$FAKE_DIR/lock" --max-wake 3s \
  --work-dir "$TBW" --host-lock "$HL" --heartbeat-every 0 --work-backoff off >/dev/null 2>&1; rc=$?
left=no; [ -e "$HL" ] && left="yes ($(ls -A "$HL" | tr '\n' ' '))"
ok=1; [ "$rc" -eq 0 ] && [ "$left" = no ] && ok=0
case_result "TB10 budget stop: run releases its lock cleanly, no lock dir left" $ok "rc=$rc, lock left=$left"
# TB11: a lock of a detached job is NOT released by the budget stop, and the job lives on
new_case tb11 '0||' '0||'
echo '0|demo/app#12                        In Progress      P1  order 1' >"$FAKE_DIR/plan.mine"
HL="$FAKE_DIR/heavy.lock.d"
WAKE5="bash \"$DRV/host-lock.sh\" run \"$HL\" --holder dev@host-a --no-board --detach --result \"$FAKE_DIR/res\" -- bash -c 'sleep 7; echo jobdone'; sleep 300"
CREWBOARD_KILL_GRACE=2 "$P" --agent dev@host-a --wake-cmd "$WAKE5" --lock-dir "$FAKE_DIR/lock" --max-wake 3s \
  --work-dir "$TBW" --host-lock "$HL" --heartbeat-every 0 --work-backoff off >/dev/null 2>&1; rc=$?
jp=$(sed -n 's/^pid=//p' "$HL/owner" 2>/dev/null)
alive=no; [ -n "$jp" ] && kill -0 "$jp" 2>/dev/null && alive=yes
ok=1; [ "$rc" -eq 0 ] && [ "$alive" = yes ] && grep -q '^detached=1$' "$HL/owner" 2>/dev/null && [ ! -e "$FAKE_DIR/res" ] \
  && grep -q "host lock $HL: kept for dev@host-a" "$FAKE_DIR/wake.log" && ok=0
case_result "TB11 budget stop: detached job and its lock stay" $ok "rc=$rc, job pid=$jp alive=$alive"
i=0; while [ ! -e "$FAKE_DIR/res" ] && [ "$i" -lt 150 ]; do sleep 0.1; i=$((i + 1)); done
j=0; while [ -e "$HL" ] && [ "$j" -lt 30 ]; do sleep 0.1; j=$((j + 1)); done
ok=1; grep -q '^rc=0 ' "$FAKE_DIR/res" 2>/dev/null && [ ! -e "$HL" ] && ok=0
case_result "TB11b the detached job ends after the stop: result, lock released" $ok "result=$(cat "$FAKE_DIR/res" 2>/dev/null | cut -c1-30)"
# TB3: a bad --max-wake flag is a usage error (64) before any poll; 1m30s style is not a duration
new_case tb3 '0||'
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --max-wake soon >/dev/null 2>&1; r1=$?
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --max-wake 1m30s >/dev/null 2>&1; r2=$?
ok=1; [ "$r1" -eq 64 ] && [ "$r2" -eq 64 ] && [ "$(ncalls)" -eq 0 ] && ok=0
case_result "TB3 bad --max-wake -> 64, no poll" $ok "rc=$r1/$r2, calls=$(ncalls)"

# TB4 to TB7: where the work of the ticket is, and no override line in the message.
# A crew-like setup: a bare remote, a work dir with one clone under it on the ticket branch.
TBR="$T/tb157-remote.git"; git -c init.defaultBranch=main init -q --bare "$TBR"
TBP="$T/tb157-parent"; mkdir -p "$TBP"
git -c init.defaultBranch=main clone -q "$TBR" "$TBP/app" 2>/dev/null
git -C "$TBP/app" -c user.email=t@x -c user.name=t commit -q --allow-empty -m one
git -C "$TBP/app" push -q origin HEAD:main 2>/dev/null
git -C "$TBP/app" checkout -q -b crew/dev@host-a/12-fix
git -C "$TBP/app" -c user.email=t@x -c user.name=t commit -q --allow-empty -m two
git -C "$TBP/app" push -q -u origin crew/dev@host-a/12-fix 2>/dev/null
TBP_HEAD=$(git -C "$TBP/app" rev-parse HEAD)
budget_stop() { # work dir, then more options: one wake stopped at a 1 s budget
  local wd=$1; shift
  CREWBOARD_KILL_GRACE=1 "$P" --agent dev@host-a --wake-cmd 'sleep 30' --lock-dir "$FAKE_DIR/lock" --max-wake 1s \
    --work-dir "$wd" --heartbeat-every 0 --work-backoff off "$@" >/dev/null 2>&1
}
said() { grep '^say ' "$FAKE_DIR/calls" | sed 's/.* --body //'; }
MINE_PR='0|demo/app#12                        In Progress      P1  order 1\nnow: work to do\n  demo/app#12                        work: CI of demo/app#45 is green: deliver with `done`'
# TB4: a pushed ticket branch in a clone under the work dir, and the PR that `mine` names:
# branch and head SHA, pushed, PR. Without an events file the say keeps --override-unread.
new_case tb4 '0||' '0||'
printf '%s\n' "$MINE_PR" >"$FAKE_DIR/plan.mine"
echo "crew/dev@host-a/12-fix $TBP_HEAD" >"$FAKE_DIR/gh.view"
budget_stop "$TBP"; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(said)" = "stopped at the time budget (1s) of the wake wrapper. Ticket demo/app#12, branch crew/dev@host-a/12-fix, head $TBP_HEAD (pushed, PR demo/app#45). The next wake continues from here." ] \
  && grep -qx 'pr view 45 --repo demo/app --json headRefName,headRefOid --jq .headRefName + " " + .headRefOid' "$FAKE_DIR/gh.calls" && ok=0
case_result "TB4 pushed ticket branch: branch, head SHA, pushed, PR" $ok "said='$(said | cut -c60-200)'"
# TB5: no PR named by mine: the open PR of the ticket branch from gh pr list (not the decoy of
# another agent); the work dir itself is no checkout
new_case tb5 '0||' '0||'
echo '0|demo/app#12                        In Progress      P1  order 1' >"$FAKE_DIR/plan.mine"
printf '46 crew/other@host-b/12-fix 1111111\n45 crew/dev@host-a/12-fix 2222222\n' >"$FAKE_DIR/gh.list"
mkdir -p "$FAKE_DIR/nogit"
budget_stop "$FAKE_DIR/nogit"; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(said)" = "stopped at the time budget (1s) of the wake wrapper. Ticket demo/app#12, branch crew/dev@host-a/12-fix, head 2222222 (pushed, PR demo/app#45; the work dir is not on that branch). The next wake continues from here." ] \
  && grep -q "no git checkout in the work dir $FAKE_DIR/nogit" "$FAKE_DIR/wake.log" && ok=0
case_result "TB5 PR of the ticket branch from gh pr list, decoy skipped" $ok "said='$(said | cut -c60-200)'"
# TB6: the other direction: no ticket branch and no PR -> the message says so (no "unknown")
new_case tb6 '0||' '0||'
echo '0|demo/app#12                        In Progress      P1  order 1' >"$FAKE_DIR/plan.mine"
mkdir -p "$FAKE_DIR/nogit2"
budget_stop "$FAKE_DIR/nogit2"; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(said)" = "stopped at the time budget (1s) of the wake wrapper. Ticket demo/app#12, no branch and no PR of the ticket found (work dir $FAKE_DIR/nogit2). The next wake continues from here." ] \
  && ! said | grep -q unknown && ok=0
case_result "TB6 no branch, no PR: the message says so" $ok "said='$(said | cut -c60-200)'"
# TB7: with an events file (agent-wake), the unread events are read first: poll before say, no
# --override-unread (so crewboard adds no override line), the events go to <file>.unhandled, and the next
# run wakes for them although poll --quiet finds none
new_case tb7 '0||' '0||' '0|MESSAGE from coordinator on #12: please rebase|' '0||' '1||' '0||' '1||'
printf '%s\n' "$MINE_PR" "$MINE_PR" "$MINE_PR" >"$FAKE_DIR/plan.mine"   # events file, budget end, next wake
echo "crew/dev@host-a/12-fix $TBP_HEAD" >"$FAKE_DIR/gh.view"
budget_stop "$TBP" --events-file "$FAKE_DIR/ev.txt"; rc1=$?
order=$(grep -n -E '^(poll|say)( |$)' "$FAKE_DIR/calls" | cut -c1-14 | tr '\n' ' ')
sayl=$(grep '^say ' "$FAKE_DIR/calls")
kept=$(grep -c 'please rebase' "$FAKE_DIR/ev.txt.unhandled" 2>/dev/null)
"$P" --agent dev@host-a --wake-cmd 'cp "$CREWBOARD_EVENTS_FILE" "$FAKE_DIR/seen"' --lock-dir "$FAKE_DIR/lock" \
  --events-file "$FAKE_DIR/ev.txt" --heartbeat-every 0 --work-backoff off --max-wake off >/dev/null 2>&1; rc2=$?
ok=1; [ "$rc1" -eq 0 ] && [ "$order" = "1:poll --quiet 2:poll 3:poll 4:say demo/app " ] \
  && ! printf '%s' "$sayl" | grep -q -- '--override-unread' && ! printf '%s' "$sayl" | grep -q 'override' \
  && [ "$kept" = 1 ] && [ "$rc2" -eq 0 ] && grep -q 'please rebase' "$FAKE_DIR/seen" && [ ! -f "$FAKE_DIR/ev.txt.unhandled" ] && ok=0
case_result "TB7 events read before say: no --override-unread; next wake gets them" $ok "order=$order kept=$kept rc=$rc1/$rc2"

# ---------------- events for the model (move 4) ----------------
echo "== events file"
# EV1: agent-wake writes the events (poll, which moves the cursor) and mine to a file in the state dir
# before the model starts; the prompt names the file; the model gets CREWBOARD_EVENTS_FILE
new_case ev1 '0||' '0|EVENT 1: assign #12 to dev@host-a|'
echo '0|demo/app#12                        Ready            P1  order 1' >"$FAKE_DIR/plan.mine"
"$AWX" --runtime claude "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
EVF="$FAKE_DIR/events.txt"
ok=1; [ "$rc" -eq 0 ] && grep -q 'EVENT 1: assign #12 to dev@host-a' "$EVF" && grep -q '^demo/app#12 ' "$EVF" \
  && [ "$(sed -n 1p "$FAKE_DIR/calls")" = "poll --quiet --initiative demo" ] && [ "$(sed -n 2p "$FAKE_DIR/calls")" = "poll --initiative demo" ] \
  && grep -q "^arg=.*Your events are in $EVF; handle them\." "$FAKE_DIR/rt.out" && grep -qx "events=$EVF" "$FAKE_DIR/rt.out" \
  && ! grep -q 'Run crewboard watch --timeout 1m, handle every event' "$FAKE_DIR/rt.out" && ok=0
case_result "EV1 events and mine in a file; the prompt names it" $ok "rc=$rc, calls=$(ncalls), file lines=$(nlines "$EVF")"
# EV2: a failed wake keeps its events; the next wake gets them at the top, with a warning
new_case ev2 '0||' '0|EVENT 7: question on #4|' '0||' '1||'
"$P" --wake-cmd 'exit 9' --lock-dir "$FAKE_DIR/lock" --events-file "$FAKE_DIR/ev.txt" --heartbeat-every 0 >/dev/null 2>&1; rc1=$?
kept=$([ -s "$FAKE_DIR/ev.txt.unhandled" ] && echo yes || echo no)
"$P" --wake-cmd 'cp "$CREWBOARD_EVENTS_FILE" "$FAKE_DIR/seen"' --lock-dir "$FAKE_DIR/lock" --events-file "$FAKE_DIR/ev.txt" --heartbeat-every 0 >/dev/null 2>&1; rc2=$?
ok=1; [ "$rc1" -eq 70 ] && [ "$kept" = yes ] && [ "$rc2" -eq 0 ] && grep -q 'An earlier wake ended early' "$FAKE_DIR/seen" \
  && grep -q 'EVENT 7: question on #4' "$FAKE_DIR/seen" && grep -q '(no new events)' "$FAKE_DIR/seen" && [ ! -f "$FAKE_DIR/ev.txt.unhandled" ] && ok=0
case_result "EV2 failed wake keeps its events for the next wake" $ok "rc=$rc1/$rc2, kept=$kept"
# EV3: the other direction: without --events-file only `poll --quiet` runs (the cursor stays)
new_case ev3 '0||'
"$P" --wake-cmd "$WAKE" --lock-dir "$FAKE_DIR/lock" --heartbeat-every 0 >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(ncalls)" -eq 1 ] && [ "$(cat "$FAKE_DIR/calls")" = "poll --quiet" ] && ok=0
case_result "EV3 no --events-file: only poll --quiet, the cursor stays" $ok "calls=$(ncalls)"

# ---------------- event receipts ----------------
echo "== event receipts"
ACTLOG() { cat "$FAKE_DIR/wake.log" 2>/dev/null; }
acw() { # events file, wake command: one wake with the events file
  "$P" --wake-cmd "$2" --lock-dir "$FAKE_DIR/lock" --events-file "$1" --heartbeat-every 0 --work-backoff off >/dev/null 2>&1
}
# AC1: every event of the wake gets one receipt line in the log; a wake that acted gets no
# re-delivery, and the check gets the wake start and the file of the next wake
new_case ac1 '0||' '0|EVENT 1: assign #12|'
printf '%s\n' 'demo/app#12 assignment' 'demo/app#7 info' 'owner-message' >"$FAKE_DIR/acted.list"
acw "$FAKE_DIR/ev.txt" 'exit 0'; rc=$?
lg=$(ACTLOG)
ok=1; [ "$rc" -eq 0 ] && printf '%s\n' "$lg" | grep -q 'poll-and-wake: event delivered to wake: demo/app#12 assignment$' \
  && printf '%s\n' "$lg" | grep -q 'poll-and-wake: event delivered to wake: demo/app#7 info$' \
  && printf '%s\n' "$lg" | grep -q 'poll-and-wake: event delivered to wake: owner-message$' \
  && ! printf '%s\n' "$lg" | grep -q 'not acted on' && [ ! -e "$FAKE_DIR/ev.txt.unhandled" ] \
  && [ "$(nlines "$FAKE_DIR/calls.acted")" -eq 2 ] \
  && sed -n 2p "$FAKE_DIR/calls.acted" | grep -q -- "^acted --events-file $FAKE_DIR/ev.txt --since 20[0-9-]*T[0-9:]*Z --redeliver-to $FAKE_DIR/ev.txt.unhandled" \
  && [ "$(ncalls)" -eq 2 ] && ok=0
case_result "AC1 receipts logged; a wake that acted: no re-delivery" $ok "rc=$rc, acted calls=$(nlines "$FAKE_DIR/calls.acted"), poll calls=$(ncalls)"
# AC2: exit 0 without an act: the event goes to .unhandled ONCE (marker) and the log says so; the
# next wake gets it again at the top of its file; a second miss only logs
new_case ac2 '0||' '0|EVENT 1: ask on #12|' '1||' '1||'
echo 'demo/app#12 question' >"$FAKE_DIR/acted.list"; echo 'demo/app#12 question' >"$FAKE_DIR/acted.check"
acw "$FAKE_DIR/ev.txt" 'exit 0'; rc1=$?
lg=$(ACTLOG); n1=$(grep -c 'REDELIVERED demo/app#12 question' "$FAKE_DIR/ev.txt.unhandled" 2>/dev/null)
printf '%s\n' "$lg" | grep -q 'poll-and-wake: event not acted on: demo/app#12 question; delivered again$'; l1=$?
acw "$FAKE_DIR/ev.txt" 'cp "$CREWBOARD_EVENTS_FILE" "$FAKE_DIR/seen"'; rc2=$?
lg=$(ACTLOG)
ok=1; [ "$rc1" -eq 0 ] && [ "$rc2" -eq 0 ] && [ "$n1" = 1 ] && [ "$l1" -eq 0 ] \
  && grep -q 'An earlier wake ended early' "$FAKE_DIR/seen" && grep -qx 'REDELIVERED demo/app#12 question' "$FAKE_DIR/seen" \
  && printf '%s\n' "$lg" | grep -q 'poll-and-wake: event not acted on twice: demo/app#12 question; not delivered again$' \
  && [ "$(printf '%s\n' "$lg" | grep -c 'event not acted on: ')" -eq 1 ] && [ ! -e "$FAKE_DIR/ev.txt.unhandled" ] && ok=0
case_result "AC2 one miss: delivered again once; second miss: only the log" $ok "rc=$rc1/$rc2, unhandled copies=$n1"
# AC3: the failing wake keeps its old path (the whole file to .unhandled, exit 70) and runs no check
new_case ac3 '0||' '0|EVENT 1: ask on #12|'
echo 'demo/app#12 question' >"$FAKE_DIR/acted.list"; echo 'demo/app#12 question' >"$FAKE_DIR/acted.check"
acw "$FAKE_DIR/ev.txt" 'exit 9'; rc=$?
ok=1; [ "$rc" -eq 70 ] && grep -q 'EVENT 1: ask on #12' "$FAKE_DIR/ev.txt.unhandled" && ! grep -q REDELIVERED "$FAKE_DIR/ev.txt.unhandled" \
  && [ "$(nlines "$FAKE_DIR/calls.acted")" -eq 1 ] && ! ACTLOG | grep -q 'not acted on' && ok=0
case_result "AC3 failing wake: old path, no check" $ok "rc=$rc, acted calls=$(nlines "$FAKE_DIR/calls.acted")"
# AC4: the budget stop keeps its old path and runs no check
new_case ac4 '0||' '0||' '0||'
echo 'demo/app#12 question' >"$FAKE_DIR/acted.list"; echo 'demo/app#12 question' >"$FAKE_DIR/acted.check"
budget_stop "$FAKE_DIR/nogit" --events-file "$FAKE_DIR/ev.txt"; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(nlines "$FAKE_DIR/calls.acted")" -eq 1 ] && ! ACTLOG | grep -q 'not acted on' && [ ! -e "$FAKE_DIR/ev.txt.unhandled" -o -z "$(grep REDELIVERED "$FAKE_DIR/ev.txt.unhandled" 2>/dev/null)" ] && ok=0
case_result "AC4 budget stop: old path, no check" $ok "rc=$rc, acted calls=$(nlines "$FAKE_DIR/calls.acted")"
# AC5: a check that fails (an older crewboard without `acted`) logs one line each and changes nothing
new_case ac5 '0||' '0|EVENT 1: ask on #12|'
echo 2 >"$FAKE_DIR/acted.rc"
acw "$FAKE_DIR/ev.txt" 'exit 0'; rc=$?
ok=1; [ "$rc" -eq 0 ] && ACTLOG | grep -q 'event receipts not written: crewboard acted exit 2' \
  && ACTLOG | grep -q 'event check failed: crewboard acted exit 2' && ACTLOG | grep -q 'wake finished OK' \
  && [ ! -e "$FAKE_DIR/ev.txt.unhandled" ] && ok=0
case_result "AC5 failing check: logged, nothing delivered again, rc 0" $ok "rc=$rc"
# AC6: without --events-file there is no receipt and no check
new_case ac6 '0||'
"$P" --wake-cmd 'exit 0' --lock-dir "$FAKE_DIR/lock" --heartbeat-every 0 >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ ! -e "$FAKE_DIR/calls.acted" ] && ok=0
case_result "AC6 no --events-file: no acted call" $ok "rc=$rc"

# ---------------- finished detached jobs wake the agent once ----------------
# The state dir is the dir of --lock-dir; the results of this agent are in <state dir>/results/.
RSW='echo wake >>"$FAKE_DIR/wakes"; cp "$CREWBOARD_EVENTS_FILE" "$FAKE_DIR/seen"; echo "dir=$CREWBOARD_RESULTS_DIR" >"$FAKE_DIR/rdir"'
rsp() { "$P" --wake-cmd "$RSW" --lock-dir "$FAKE_DIR/lock" --events-file "$FAKE_DIR/ev.txt" --heartbeat-every 0 --work-backoff off >/dev/null 2>&1; }
# RS1: a finished result wakes the quiet poll once, with the event line; the next poll is quiet
new_case rs1
mkdir -p "$FAKE_DIR/results"
printf 'rc=0 end=2026-10-21T10:00:00Z log=%s/results/7.result.log\nticket=7\n' "$FAKE_DIR" >"$FAKE_DIR/results/7.result"
rsp; rc1=$?; w1=$(nlines "$FAKE_DIR/wakes")
rm -f "$FAKE_DIR/seen"; rsp; rc2=$?; w2=$(nlines "$FAKE_DIR/wakes")
ok=1; [ "$rc1" -eq 0 ] && [ "$w1" -eq 1 ] && [ "$rc2" -eq 0 ] && [ "$w2" -eq 1 ] && [ ! -f "$FAKE_DIR/seen" ] \
  && grep -qx "dir=$FAKE_DIR/results" "$FAKE_DIR/rdir" && ok=0
case_result "RS1 finished detached job: quiet poll wakes once, then quiet" $ok "wakes=$w1/$w2"
ok=1; grep -qx "detached job finished: ticket 7 rc=0 log=$FAKE_DIR/results/7.result.log result=$FAKE_DIR/results/7.result" "$FAKE_DIR/ev.txt" && ok=0
case_result "RS1b the events file has the line of the finished job" $ok "$(grep 'detached job' "$FAKE_DIR/ev.txt")"
# RS1c: a new run of the same job (a new rc line) is reported again; a result without ticket= still works
printf 'rc=1 end=2026-10-21T11:00:00Z log=%s/results/7.result.log\n' "$FAKE_DIR" >"$FAKE_DIR/results/7.result"
rsp; w3=$(nlines "$FAKE_DIR/wakes")
ok=1; [ "$w3" -eq 2 ] && grep -q 'detached job finished: ticket unknown rc=1 ' "$FAKE_DIR/ev.txt" && ok=0
case_result "RS1c a new result of the same file is reported; no ticket= is fine" $ok "wakes=$w3"
# RS2: an unfinished result (no rc= line) is not reported; next to a finished one only that one is named
new_case rs2
mkdir -p "$FAKE_DIR/results"
printf 'ticket=8\n' >"$FAKE_DIR/results/8.result"
rsp; rc0=$?; w0=$(nlines "$FAKE_DIR/wakes")
printf 'rc=0 end=2026-10-21T10:00:00Z log=a.log\nticket=10\n' >"$FAKE_DIR/results/10.result"
rsp; rc=$?
ok=1; [ "$rc0" -eq 0 ] && [ "$w0" -eq 0 ] && [ "$(nlines "$FAKE_DIR/wakes")" -eq 1 ] && grep -q 'ticket 10 rc=0' "$FAKE_DIR/ev.txt" \
  && ! grep -q 'ticket 8' "$FAKE_DIR/ev.txt" && [ ! -f "$FAKE_DIR/results/8.result.seen" ] && ok=0
case_result "RS2 unfinished result: not reported, no wake alone" $ok "wakes before=$w0, after=$(nlines "$FAKE_DIR/wakes")"
# RS3: a result in another dir (another agent's) is not read
new_case rs3
mkdir -p "$FAKE_DIR/other/results" "$FAKE_DIR/results"
printf 'rc=0 end=2026-10-21T10:00:00Z log=x.log\nticket=9\n' >"$FAKE_DIR/other/results/9.result"
rsp; w0=$(nlines "$FAKE_DIR/wakes")
printf 'rc=3 end=2026-10-21T10:00:00Z log=y.log\nticket=11\n' >"$FAKE_DIR/results/11.result"
rsp
ok=1; [ "$w0" -eq 0 ] && [ "$(nlines "$FAKE_DIR/wakes")" -eq 1 ] && grep -q 'ticket 11 rc=3' "$FAKE_DIR/ev.txt" \
  && ! grep -q 'ticket 9' "$FAKE_DIR/ev.txt" && [ ! -f "$FAKE_DIR/other/results/9.result.seen" ] && ok=0
case_result "RS3 result of another dir: not read" $ok "wakes before=$w0, after=$(nlines "$FAKE_DIR/wakes")"
# RS4: a script in the results dir with `rc=` at column 0 is not a result; a real result still is
new_case rs4
mkdir -p "$FAKE_DIR/results"
printf '#!/bin/bash\nrun-chain 2>&1 | tee chain.log\nrc=${PIPESTATUS[0]}\nexit "$rc"\n' >"$FAKE_DIR/results/239-chain.sh"
rsp; w0=$(nlines "$FAKE_DIR/wakes")
printf 'rc=0 end=2026-10-21T10:00:00Z log=z.log\nticket=12\n' >"$FAKE_DIR/results/12.result"
rsp
ok=1; [ "$w0" -eq 0 ] && [ "$(nlines "$FAKE_DIR/wakes")" -eq 1 ] && grep -q 'ticket 12 rc=0' "$FAKE_DIR/ev.txt" \
  && ! grep -q '239-chain' "$FAKE_DIR/ev.txt" && [ ! -f "$FAKE_DIR/results/239-chain.sh.seen" ] && ok=0
case_result "RS4 script with rc= at column 0: not reported; a real result still is" $ok "wakes before=$w0, after=$(nlines "$FAKE_DIR/wakes")"
# ---------------- the events hook: `crewboard hook post-tool-use` ----------------
echo "== events hook"
HK="$T/bin/events-hook"
printf '%s\n' '#!/bin/sh' "exec '$CREWBOARD_REAL' hook post-tool-use \"\$@\"" >"$HK"; chmod +x "$HK"
hk() { # run the hook as Claude Code does: JSON on stdin, the events file in the environment
  CREWBOARD_EVENTS_FILE="$FAKE_DIR/events.txt" "$HK" --crewboard crewboard --initiative demo --log "$FAKE_DIR/hook.log" "$@" \
    <<<'{"hook_event_name":"PostToolUse"}' >"$FAKE_DIR/hook.out" 2>"$FAKE_DIR/hook.err"
}
# HK1: not due -> no poll, no output
new_case hk1 '0|EVENT 1: owner merged #12|'
echo "== Events:" >"$FAKE_DIR/events.txt"; date +%s >"$FAKE_DIR/events.txt.hook-stamp"
hk --every 300; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(ncalls)" -eq 0 ] && [ ! -s "$FAKE_DIR/hook.out" ] && [ "$(nlines "$FAKE_DIR/events.txt")" -eq 1 ] && ok=0
case_result "HK1 hook not due: no poll, no output, events file unchanged" $ok "rc=$rc, calls=$(ncalls)"
# HK1b: no stamp, but the loop wrote the events file just now (its own poll) -> not due
new_case hk1b '0|EVENT 1: owner merged #12|'
echo "== Events:" >"$FAKE_DIR/events.txt"; rm -f "$FAKE_DIR/events.txt.hook-stamp"
hk --every 300; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(ncalls)" -eq 0 ] && [ ! -s "$FAKE_DIR/hook.out" ] && ok=0
case_result "HK1b no stamp, fresh events file: the poll of the loop counts, no poll" $ok "rc=$rc, calls=$(ncalls)"
# HK2: due, poll has events -> valid JSON with additionalContext, events appended, stamp written
new_case hk2 '0|EVENT 1: owner merged #12 "now"\\nEVENT 2: a\\tb|'
echo "== Events:" >"$FAKE_DIR/events.txt"; echo 0 >"$FAKE_DIR/events.txt.hook-stamp"   # the last poll is long ago
hk --every 300 --config-dir "$T/cfg"; rc=$?
ctx=$(jq -r '.hookSpecificOutput.additionalContext' "$FAKE_DIR/hook.out" 2>/dev/null)
name=$(jq -r '.hookSpecificOutput.hookEventName' "$FAKE_DIR/hook.out" 2>/dev/null)
first=$(printf '%s\n' "$ctx" | sed -n 1p)
ok=1; [ "$rc" -eq 0 ] && [ "$name" = PostToolUse ] && [ "$first" = "crewboard: new events since this wake started:" ] \
  && printf '%s' "$ctx" | grep -q 'EVENT 1: owner merged #12 "now"' && printf '%s' "$ctx" | grep -q 'EVENT 2: a' \
  && grep -q '^== Events delivered during the wake (crewboard poll, ' "$FAKE_DIR/events.txt" && grep -q 'EVENT 2: a' "$FAKE_DIR/events.txt" \
  && [ "$(cat "$FAKE_DIR/calls")" = "--config-dir $T/cfg poll --initiative demo" ] && [ -s "$FAKE_DIR/events.txt.hook-stamp" ] && ok=0
case_result "HK2 hook due, events: JSON additionalContext, events file, same poll call" $ok "rc=$rc, name=$name, calls=$(cat "$FAKE_DIR/calls")"
# HK2b: right after, the hook is not due again
hk --every 300; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(ncalls)" -eq 1 ] && [ ! -s "$FAKE_DIR/hook.out" ] && ok=0
case_result "HK2b the next call within the interval: no second poll" $ok "calls=$(ncalls)"
# HK3: due, poll exit 1 (no events) -> no output, nothing appended, exit 0
new_case hk3 '1||'
echo "== Events:" >"$FAKE_DIR/events.txt"
hk --every 0; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(ncalls)" -eq 1 ] && [ ! -s "$FAKE_DIR/hook.out" ] && [ "$(nlines "$FAKE_DIR/events.txt")" -eq 1 ] && ok=0
case_result "HK3 hook due, no events: no output, exit 0" $ok "rc=$rc"
# HK4: due, poll fails (exit 3, 4, 7) -> no output, exit 0, one log line
ok=0; det=""
for code in 3 4 7; do
  new_case hk4 "$code||HTTP 502"
  echo "== Events:" >"$FAKE_DIR/events.txt"
  hk --every 0; rc=$?
  [ "$rc" -eq 0 ] && [ ! -s "$FAKE_DIR/hook.out" ] && [ "$(nlines "$FAKE_DIR/events.txt")" -eq 1 ] \
    && grep -q "poll failed with exit $code" "$FAKE_DIR/hook.log" && [ "$(nlines "$FAKE_DIR/hook.log")" -eq 1 ] || ok=1
  det="$det rc$code=$rc"
done
case_result "HK4 hook due, poll fails: no output, exit 0, one log line" $ok "$det"
# HK4b: no crewboard binary at all -> still exit 0, no output
new_case hk4b
CREWBOARD_EVENTS_FILE="$FAKE_DIR/events.txt" "$HK" --crewboard "$T/no-such-binary" --every 0 --log "$FAKE_DIR/hook.log" </dev/null >"$FAKE_DIR/hook.out" 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ ! -s "$FAKE_DIR/hook.out" ] && ok=0
case_result "HK4b hook with a missing crewboard binary: no output, exit 0" $ok "rc=$rc"
# HK5: not in a wake (no events file in the environment) -> nothing at all
new_case hk5 '0|EVENT 1|'
env -u CREWBOARD_EVENTS_FILE "$HK" --every 0 </dev/null >"$FAKE_DIR/hook.out" 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(ncalls)" -eq 0 ] && [ ! -s "$FAKE_DIR/hook.out" ] && ok=0
case_result "HK5 hook outside a wake: no poll, no output" $ok "rc=$rc"
# HK6: the interval comes from CREWBOARD_HOOK_EVERY when --every is not given
new_case hk6 '0|EVENT 1|'
echo "== Events:" >"$FAKE_DIR/events.txt"; date +%s >"$FAKE_DIR/events.txt.hook-stamp"
CREWBOARD_HOOK_EVERY=0 hk; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(ncalls)" -eq 1 ] && jq -e . "$FAKE_DIR/hook.out" >/dev/null 2>&1 && ok=0
case_result "HK6 CREWBOARD_HOOK_EVERY=0 makes the hook due" $ok "calls=$(ncalls)"



# ---------------- headless coordinator without a model (move 2) ----------------
echo "== headless coordinator"
COC=(--role coordinator --initiative demo --env-file "$AW/env" --config-dir "$T/coord-headless" --heartbeat-every 0)
mkdir -p "$T/coord-headless"
TICKOUT='0|tick: verified and merged demo/app#5\nleft for the interactive coordinator: demo/app#6, the Gate is not CI only|'
# CO1: crewboard tick runs, its lines go to the log, no model starts, no poll runs
new_case co1 "$TICKOUT"
rm -f "$FAKE_DIR/rt.out"
"$AWX" "${COC[@]}" --lock-dir "$FAKE_DIR/lock" >"$FAKE_DIR/out" 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$(cat "$FAKE_DIR/calls")" = "--config-dir $T/coord-headless tick --initiative demo" ] \
  && [ ! -f "$FAKE_DIR/rt.out" ] && [ ! -s "$FAKE_DIR/out" ] \
  && grep -q 'poll-and-wake: tick: tick: verified and merged demo/app#5$' "$FAKE_DIR/wake.log" \
  && grep -q 'poll-and-wake: tick: left for the interactive coordinator: demo/app#6, the Gate is not CI only$' "$FAKE_DIR/wake.log" && ok=0
case_result "CO1 coordinator: tick lines logged, no model, no poll" $ok "rc=$rc, calls=$(ncalls), runtime ran=$([ -f "$FAKE_DIR/rt.out" ] && echo yes || echo no)"
# CO2: the same output again, and an idle tick, write no new line (one line per change, not per run)
printf '%s\n' "$TICKOUT" '0|tick: no ticket in Review|' >>"$FAKE_DIR/plan"
before=$(nlines "$FAKE_DIR/wake.log")
"$AWX" "${COC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc1=$?
"$AWX" "${COC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc2=$?
ok=1; [ "$rc1" -eq 0 ] && [ "$rc2" -eq 0 ] && [ "$(nlines "$FAKE_DIR/wake.log")" -eq "$before" ] && [ "$(ncalls)" -eq 3 ] && ok=0
case_result "CO2 repeated and idle ticks write no new line" $ok "log lines=$before -> $(nlines "$FAKE_DIR/wake.log")"
# CO3: a refused tick (not a headless dir) is logged with its reason and exits 2
new_case co3 '2||refused [rule=tick-headless-only] tick is for a headless config dir'
"$AWX" "${COC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 2 ] && grep -q 'crewboard tick failed: exit 2: refused \[rule=tick-headless-only\]' "$FAKE_DIR/wake.log" && ok=0
case_result "CO3 refused tick: rc 2, reason logged" $ok "rc=$rc"
o=$("$AWX" "${COC[@]}" --print-cmd 2>&1)
ok=1; printf '%s' "$o" | grep -q '^crewboard tick (no model' && ok=0
case_result "CO4 --print-cmd of the coordinator: tick, no model" $ok "$o"

# ---------------- runtime flags and the prompt template ----------------
echo "== runtime flags and prompt"
# A7: --allow appends to the default tool list of claude (B5); --allow with codex is refused
new_case a7 '0||' '1||'
"$AWX" --runtime claude "${AWC[@]}" --allow 'Bash(npm *)' --allow 'Bash(npx jest *)' --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
"$AWX" --runtime codex "${AWC[@]}" --allow 'Bash(npm *)' --print-cmd >/dev/null 2>&1; rcx=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^arg=Bash(crewboard \*),Bash(git \*),Bash(gh \*),Read,Edit,Write,Glob,Grep,.*,Bash(wc \*),Bash(npm \*),Bash(npx jest \*)$' "$FAKE_DIR/rt.out" \
  && [ "$rcx" -eq 4 ] && ok=0
case_result "A7 --allow appends to the claude tools; codex refuses it" $ok "rc=$rc, codex rc=$rcx"
# A8: codex gets --add-dir <config dir> and the lock parent of --host-lock; claude gets the state dir
new_case a8 '0||' '1||'
mkdir -p "$T/a8cfg" "$T/a8locks"
"$AWX" --runtime codex "${AWC[@]}" --config-dir "$T/a8cfg" --host-lock "$T/a8locks/heavy.lock.d" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
adds=$(grep -A1 -x 'arg=--add-dir' "$FAKE_DIR/rt.out" | grep -v -x -e 'arg=--add-dir' -e '--' | tr '\n' '|')
ok=1; [ "$rc" -eq 0 ] && [ "$adds" = "arg=$T/a8cfg|arg=$T/a8locks|" ] \
  && grep -q "^arg=.*crewboard lock run $T/a8locks/heavy.lock.d --holder dev@host-a" "$FAKE_DIR/rt.out" && ok=0
case_result "A8 codex: --add-dir config dir and lock dir; host-lock rule in prompt" $ok "rc=$rc, add-dir=$adds"
# A8b: claude gets --add-dir for the state dir AND the lock parent of --host-lock (a log there is allowed)
new_case a8b '0||' '1||'
HOME="$T/a8nohome" "$AWX" --runtime claude "${AWC[@]}" --host-lock "$T/a8locks/heavy.lock.d" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
adds=$(grep -A1 -x 'arg=--add-dir' "$FAKE_DIR/rt.out" | grep -v -x -e 'arg=--add-dir' -e '--' | tr '\n' '|')
ok=1; [ "$rc" -eq 0 ] && case "$adds" in *"arg=$T/a8locks|"*) true ;; *) false ;; esac && [ "$(printf '%s' "$adds" | tr '|' '\n' | grep -c .)" -eq 2 ] && ok=0
case_result "A8b claude: --add-dir state dir and lock dir" $ok "rc=$rc, add-dir=$adds"
# A8c: the wake marks itself and grants `crewboard lock run` for the --host-lock dirs only, so the
# model cannot run any command under a lock of its own choice; no --host-lock: nothing granted
ok=1; grep -qx 'wake=1' "$FAKE_DIR/rt.out" && grep -qx "lockdirs=$T/a8locks/heavy.lock.d" "$FAKE_DIR/rt.out" \
  && ! grep -q 'host-lock.sh' "$FAKE_DIR/rt.out" && ok=0
case_result "A8c wake sets CREWBOARD_WAKE and CREWBOARD_LOCK_RUN_DIRS from --host-lock" $ok "$(grep -E '^(wake|lockdirs)=' "$FAKE_DIR/rt.out" | tr '\n' ' ')"
new_case a8d '0||' '1||'
"$AWX" --runtime claude "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -qx 'wake=1' "$FAKE_DIR/rt.out" && grep -qx 'lockdirs=' "$FAKE_DIR/rt.out" && ok=0
case_result "A8d without --host-lock: wake=1 and no lock dir granted" $ok "rc=$rc"
# A8e: an installed skill (the shared copy is a symlink into a clone) is readable for claude: the
# real path and the claude link dir are --add-dir; the prompt carries the refused-command rule
new_case a8e '0||' '1||'
mkdir -p "$T/a8home/clone-skill" "$T/a8home/.config/agent-skills" "$T/a8home/.claude/skills"
ln -s "$T/a8home/clone-skill" "$T/a8home/.config/agent-skills/crewboard"
ln -s "$T/a8home/.config/agent-skills/crewboard" "$T/a8home/.claude/skills/crewboard"
real=$(cd "$T/a8home/clone-skill" && pwd -P)
HOME="$T/a8home" "$AWX" --runtime claude "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
adds=$(grep -A1 -x 'arg=--add-dir' "$FAKE_DIR/rt.out" | grep -v -x -e 'arg=--add-dir' -e '--' | tr '\n' '|')
ok=1; [ "$rc" -eq 0 ] && case "$adds" in *"|arg=$real|arg=$T/a8home/.claude/skills/crewboard|") true ;; *) false ;; esac \
  && grep -q 'Never stop because of one refused command' "$FAKE_DIR/rt.out" \
  && grep -q 'Never start a background process of your runtime' "$FAKE_DIR/rt.out" && ok=0
case_result "A8e claude: --add-dir skill dirs; refused-command and background rules in the prompt" $ok "rc=$rc, add-dir=$adds"
# A9: hermes gets chat -Q -s crewboard -q and the SAME prompt as claude (one template)
new_case a9 '0||' '1||'
"$AWX" --runtime claude "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1
pc=$(grep '^arg=Use the crewboard skill' "$FAKE_DIR/rt.out")
printf '0||\n1||\n' >>"$FAKE_DIR/plan"
"$AWX" --runtime hermes "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ph=$(grep '^arg=Use the crewboard skill' "$FAKE_DIR/rt.out")
args=$(grep '^arg=' "$FAKE_DIR/rt.out" | head -n 5 | tr '\n' ' ')
ok=1; [ "$rc" -eq 0 ] && [ -n "$pc" ] && [ "$pc" = "$ph" ] && [ "$args" = "arg=chat arg=-Q arg=-s arg=crewboard arg=-q " ] && ok=0
case_result "A9 hermes: chat -Q -s crewboard -q, the same prompt as claude" $ok "rc=$rc, args=$args"
# A10: the default prompt has no budget sentence any more (the wrapper does the budget end)
o=$("$AWX" --runtime claude "${AWC[@]}" --print-cmd)
ok=1; ! printf '%s' "$o" | grep -q -i 'budget' && ! printf '%s' "$o" | grep -q 'poll --quiet' \
  && printf '%s' "$o" | grep -q 'Use the crewboard skill (SKILL.md and references/dev.md)' \
  && printf '%s' "$o" | grep -q 'work as references/dev.md "Headless wake: work until done" says' && ok=0
case_result "A10 the prompt points to the skill; no budget or checkpoint copy" $ok "budget words=$(printf '%s' "$o" | grep -c -i budget)"

# HW1: agent-wake --runtime claude puts a PostToolUse hook into the wake command (--settings JSON)
new_case hw1 '0||' '1||'
"$AWX" --runtime claude "${AWC[@]}" --config-dir "$AW/agent" --crewboard "$T/bin/crewboard" --events-every 120 --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1
sj=$(grep -A1 -x 'arg=--settings' "$FAKE_DIR/rt.out" | sed -n 2p | sed 's/^arg=//')
hc=$(printf '%s' "$sj" | jq -r '.hooks.PostToolUse[0].hooks[0].command' 2>/dev/null)
ok=1; [ "$(printf '%s' "$sj" | jq -r '.hooks.PostToolUse[0].hooks[0].type' 2>/dev/null)" = command ] \
  && case "$hc" in *"hook post-tool-use"*"--initiative"*"demo"*"--every 120"*"--config-dir"*"$AW/agent"*|*"$HERE/wake-events-hook.sh"*"--initiative"*"demo"*"--every 120"*"--config-dir"*"$AW/agent"*) true ;; *) false ;; esac && ok=0
case_result "HW1 claude wake command has the PostToolUse hook" $ok "hook=${hc:0:100}"
# HW1b: the default interval is 300
o=$("$AWX" --runtime claude "${AWC[@]}" --print-cmd)
ok=1; case "$o" in *"--settings"*"hook post-tool-use"*"--every 300"*) ok=0 ;; esac
case_result "HW1b the default interval of the hook is 300 s" $ok ""
# HW2: --no-events-hook removes it
o=$("$AWX" --runtime claude "${AWC[@]}" --no-events-hook --print-cmd)
ok=1; ! printf '%s' "$o" | grep -q -e '--settings' -e 'hook post-tool-use' && printf '%s' "$o" | grep -q -e "-- 'Use the crewboard skill" && ok=0
case_result "HW2 --no-events-hook: no hook in the wake command" $ok ""
# HW3: codex and hermes have no hook
ok=0
for rt in codex hermes; do
  o=$("$AWX" --runtime $rt "${AWC[@]}" --print-cmd)
  printf '%s' "$o" | grep -q -e '--settings' -e 'hook post-tool-use' && ok=1
done
case_result "HW3 codex and hermes: no hook" $ok ""
# HW4: a bad --events-every is a config error (exit 4)
"$AWX" --runtime claude "${AWC[@]}" --events-every soon --print-cmd >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 4 ] && ok=0
case_result "HW4 --events-every needs whole seconds -> 4" $ok "rc=$rc"
# HW5: the fake claude of a real wake gets the hook settings and the events file path
new_case hw5 '0||' '1||'
"$AWX" --runtime claude "${AWC[@]}" --lock-dir "$FAKE_DIR/lock" >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -qx 'arg=--settings' "$FAKE_DIR/rt.out" && grep -q '^events=.*events.txt' "$FAKE_DIR/rt.out" && ok=0
case_result "HW5 a claude wake gets --settings and CREWBOARD_EVENTS_FILE" $ok "rc=$rc"

[ "${CB_IMPL:-shell}" = rust ] && finish   # the dry parity run ends here

# ---------------- --install and --uninstall: `crewboard wake install|uninstall` through the driver ----------------
# The units, the names and the scheduler commands are tested in Rust (tests/component/wake_install.rs and
# the unit tests of src/wake/install). These cases test the shim: it passes the options on, maps
# --role coordinator to --tick, and says once that it is deprecated.
echo "== install (shim)"
IH="$T/home-inst"; mkdir -p "$IH/.config/crewboard/agents/dev1" "$IH/.config/crewboard/agents/dev2" "$T/nvm/bin" "$T/sched"
cp "$T/bin/codex" "$T/nvm/bin/codex"; cp "$T/bin/claude" "$T/nvm/bin/claude"
# fakes of the scheduler commands: they record their arguments and change nothing
for c in systemctl launchctl loginctl; do
  printf '#!/bin/bash\necho "%s $*" >>"$FAKE_DIR/sched.calls"\n' "$c" >"$T/sched/$c"; chmod +x "$T/sched/$c"
done
inst() { # os, args...: agent-wake.sh with the fake HOME, the fake scheduler commands and the real crewboard
  local os=$1; shift
  env -u XDG_CONFIG_HOME HOME="$IH" CREWBOARD_WAKE_OS="$os" CREWBOARD_BIN="$CREWBOARD_REAL" PATH="$T/sched:$PATH" /bin/bash "$A" "$@"
}
ICFG="$IH/.config/crewboard/agents"
IA=(--agent dev@host-a --initiative demo --env-file "$AW/env" --work-dir "$AW/wd")
U="$IH/.config/systemd/user"
# IN1: --install systemd: the options reach `crewboard wake install`; the unit is enabled
new_case in1
o=$(inst Linux --install systemd --runtime codex "${IA[@]}" --config-dir "$ICFG/dev1" --runtime-bin "$T/nvm/bin/codex" --every 10m 2>"$FAKE_DIR/err"); rc=$?
W1="$ICFG/dev1/crewboard-demo-dev1-wake.sh"
ok=1; [ "$rc" -eq 0 ] && [ -x "$W1" ] && grep -q "ExecStart=/bin/bash $W1" "$U/crewboard-demo-dev1.service" \
  && grep -q 'OnUnitActiveSec=600s' "$U/crewboard-demo-dev1.timer" && grep -q -- "'--runtime-bin' '$T/nvm/bin/codex'" "$W1" \
  && grep -q "^exec '$CREWBOARD_REAL' 'wake' " "$W1" \
  && printf '%s' "$o" | grep -q "^installed crewboard-demo-dev1 (systemd, every 10m); log: $XDG_STATE_HOME/crewboard/agents/demo-dev1/wake.log" \
  && grep -qx 'systemctl --user enable crewboard-demo-dev1.timer' "$FAKE_DIR/sched.calls" && ok=0
case_result "IN1 shim --install systemd: crewboard wake install, unit enabled" $ok "rc=$rc, out=$o, err=$(cat "$FAKE_DIR/err")"
# IN2: --role coordinator becomes --tick; no runtime in the wrapper
new_case in2
o=$(inst Linux --install systemd --role coordinator --initiative demo --env-file "$AW/env" --config-dir "$ICFG/coord-headless" 2>&1); rc=$?
WC="$ICFG/coord-headless/crewboard-demo-coord-headless-wake.sh"
ok=1; [ "$rc" -eq 0 ] && grep -q "'wake' '--tick' '--initiative' 'demo'" "$WC" && ! grep -q -- '--runtime' "$WC" && ok=0
case_result "IN2 shim --install --role coordinator: --tick, no runtime" $ok "rc=$rc, out=$o"
# IN3: --uninstall: the unit is disabled and removed, the log stays
new_case in3
inst Linux --uninstall systemd --initiative demo --config-dir "$ICFG/dev1" >"$FAKE_DIR/out" 2>"$FAKE_DIR/err"; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -qx 'systemctl --user disable --now crewboard-demo-dev1.timer' "$FAKE_DIR/sched.calls" \
  && [ ! -f "$U/crewboard-demo-dev1.timer" ] && [ ! -f "$W1" ] && [ -f "$U/crewboard-demo-coord-headless.timer" ] \
  && ok=0
case_result "IN3 shim --uninstall removes only that agent's unit" $ok "rc=$rc, out=$(cat "$FAKE_DIR/out")"
# IN4: a wrong scheduler or option is refused by `crewboard wake install`, nothing is written
new_case in4
inst Linux --install upstart --runtime codex "${IA[@]}" --config-dir "$ICFG/dev2" --runtime-bin "$T/nvm/bin/codex" >/dev/null 2>&1; r1=$?
inst Linux --install systemd --runtime codex "${IA[@]}" --config-dir "$ICFG/dev2" --runtime-bin "$T/nope/codex" >/dev/null 2>&1; r2=$?
ok=1; [ "$r1" -ne 0 ] && [ "$r2" -eq 4 ] && [ ! -f "$U/crewboard-demo-dev2.timer" ] && [ ! -f "$ICFG/dev2/crewboard-demo-dev2-wake.sh" ] && ok=0
case_result "IN4 shim --install: a wrong scheduler or runtime is refused, nothing written" $ok "rc=$r1/$r2"
# IN11: every other mode says why when the binary is missing, or has no wake command
new_case in11
o1=$(env -i HOME="$IH" FAKE_DIR="$FAKE_DIR" PATH="$T/sched:/usr/bin:/bin" /bin/bash "$A" --runtime codex "${IA[@]}" --config-dir "$ICFG/dev1" --runtime-bin "$T/nvm/bin/codex" --print-cmd 2>&1); r1=$?
mkdir -p "$T/old"; printf '%s\n' '#!/bin/sh' 'echo "error: unrecognized subcommand wake" >&2; exit 2' >"$T/old/crewboard"; chmod +x "$T/old/crewboard"
o2=$(inst Linux --install systemd --runtime codex "${IA[@]}" --config-dir "$ICFG/dev1" --runtime-bin "$T/nvm/bin/codex" --crewboard "$T/old/crewboard" 2>&1); r2=$?
ok=1; [ "$r1" -eq 127 ] && printf '%s' "$o1" | grep -q 'poll-and-wake: crewboard not found on PATH' \
  && [ "$r2" -eq 4 ] && printf '%s' "$o2" | grep -q "agent-wake: crewboard too old: needs 'wake install'" \
  && [ ! -f "$U/crewboard-demo-dev1.timer" ] && ok=0
case_result "IN11 no binary / a binary without wake: a clear line, exit 4, nothing written" $ok "rc=$r1/$r2, $o1 / $o2"
# IN12: --print-cmd needs no env file; without HOME the message names HOME
new_case in15
o1=$(inst Linux --runtime codex --agent dev@host-a --initiative demo --env-file "$T/no-such.env" --work-dir "$AW/wd" --config-dir "$ICFG/dev1" --runtime-bin "$T/nvm/bin/codex" --print-cmd 2>&1); r1=$?
o2=$(env -i FAKE_DIR="$FAKE_DIR" CREWBOARD_REAL="$CREWBOARD_REAL" T="$T" PATH="$T/bin:/usr/bin:/bin" /bin/bash "$A" --runtime codex --agent dev@host-a --initiative demo --env-file "$AW/env" --work-dir "$AW/wd" --config-dir "$ICFG/dev1" --runtime-bin "$T/nvm/bin/codex" --print-cmd 2>&1); r2=$?
ok=1; [ "$r1" -eq 0 ] && printf '%s' "$o1" | grep -q "^cd $AW/wd && $T/nvm/bin/codex exec " \
  && [ "$r2" -eq 4 ] && printf '%s' "$o2" | grep -q 'HOME is not set' && ok=0
case_result "IN12 --print-cmd needs no env file; no HOME: the message names HOME" $ok "rc=$r1/$r2, $o1 / $o2"

finish
