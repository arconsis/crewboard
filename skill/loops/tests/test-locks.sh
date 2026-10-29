#!/bin/bash
# test-locks.sh - proof for the host lock and the gate.
# The host lock is `crewboard lock` and the gate is `crewboard repeat`; host-lock.sh and
# gate-loop.sh are shims that call them. This suite runs the
# Rust code through the shim: the binary to test is `$CREWBOARD_REAL`, or the `crewboard` on the PATH.
# No network, no tokens. A FAKE `crewboard` (stub on PATH) answers `status --json`, and hands every
# `lock` call to the real binary with `--crewboard <stub>` (the hidden test seam: the board check
# asks the stub instead of the board).
# Every case proves both directions: a lock or a run that must pass, and one that must fail.
# Usage: bash skill/loops/tests/test-locks.sh        Exit 0 only if every case passes. bash 3.2 compatible.

HERE=$(cd "$(dirname "$0")/.." && pwd)   # skill/loops/, the scripts under test
DRV="$HERE/tests/drivers"   # test drivers: thin wrappers that run the real binary the way the old scripts did
# The scripts under test live one directory up (skill/loops/). A suite that cannot find one of
# them must FAIL, not run zero cases.
for s in host-lock.sh gate-loop.sh poll-and-wake.sh; do
  [ -f "$DRV/$s" ] || { echo "FAIL  script under test not found: $DRV/$s" >&2; exit 2; }
done
# the binary under test: find it BEFORE the stub goes on the PATH
CREWBOARD_REAL=${CREWBOARD_REAL:-$(command -v crewboard)}
[ -n "$CREWBOARD_REAL" ] && [ -x "$CREWBOARD_REAL" ] || { echo "FAIL  no crewboard binary to test: set CREWBOARD_REAL or put a built crewboard on the PATH" >&2; exit 2; }
export CREWBOARD_REAL
L="$DRV/host-lock.sh"
G="$DRV/gate-loop.sh"
T=$(mktemp -d "${TMPDIR:-/tmp}/crewboard-locks.XXXXXX") || exit 1
# LIVE: a live process that is NOT an ancestor of any host-lock call. Tests that need a live
# foreign holder record it as the pid (the test shell itself is an ancestor, see H19 and up).
LIVE=$(sleep 3000 >/dev/null 2>&1 </dev/null & echo $!)   # not a job of this shell, so a bare `wait` ignores it
trap 'kill "$LIVE" 2>/dev/null; rm -rf "$T"' EXIT
mkdir -p "$T/bin"
export HOST_LOCK_HOST=hostA
export PATH="$T/bin:$PATH"
# the CI detector reads a fake process list; tests put a Runner.Worker line in it
: >"$T/ps.txt"
export HOST_LOCK_PS_CMD="cat $T/ps.txt"
RUNNER_LINE='/home/runner/actions-runner/bin/Runner.Worker spawnclient 100 101'

# the fake crewboard: `status --json` prints the file $FAKE_STATUS, or fails if it is unset
cat >"$T/bin/crewboard" <<'STUB'
#!/bin/bash
# `lock` goes to the real binary; its board check asks THIS stub (the hidden seam --crewboard).
[ "$1" = lock ] && exec "$CREWBOARD_REAL" lock --crewboard "$0" "${@:2}"
# `repeat` goes to the real binary too (gate-loop.sh is a shim for `crewboard repeat`).
[ "$1" = repeat ] && exec "$CREWBOARD_REAL" repeat "${@:2}"
# FAKE_ARGV: a file that gets the full argument line of every call. FAKE_NOCONFIG=1: fail like a
# crewboard that finds no board config unless --config-dir is among the arguments.
[ -n "$FAKE_ARGV" ] && echo "$*" >>"$FAKE_ARGV"
for a in "$@"; do
  if [ "$a" = status ]; then
    if [ -n "$FAKE_NOCONFIG" ]; then
      ok=1; for b in "$@"; do [ "$b" = --config-dir ] && ok=0; done
      [ "$ok" -eq 0 ] || { echo "error: configuration error: no board config" >&2; exit 2; }
    fi
    # FAKE_CALLS: a file that gets one line per call. FAKE_FAIL_FIRST=N: the first N calls fail.
    [ -n "$FAKE_CALLS" ] && echo call >>"$FAKE_CALLS"
    if [ -n "$FAKE_FAIL_FIRST" ] && [ "$(wc -l <"$FAKE_CALLS")" -le "$FAKE_FAIL_FIRST" ]; then
      echo "error: API rate limit exceeded" >&2; echo "second line of detail" >&2; exit 1
    fi
    [ -n "$FAKE_STATUS" ] && [ -f "$FAKE_STATUS" ] || exit 3
    /bin/cat "$FAKE_STATUS"; exit 0
  fi
done
exit 1
STUB
chmod +x "$T/bin/crewboard"

FAIL=0
case_result() { # name, ok(0/1), detail
  CASES=$((CASES + 1))
  if [ "$2" -eq 0 ]; then printf 'PASS  %-62s %s\n' "$1" "$3"; else printf 'FAIL  %-62s %s\n' "$1" "$3"; FAIL=1; fi
}
dead_pid() { sleep 0 & local p=$!; wait "$p" 2>/dev/null; echo "$p"; }
plant() { # dir agent host pid epoch
  mkdir -p "$1"
  printf 'agent=%s\nhost=%s\npid=%s\nstarted=planted\nstarted_epoch=%s\nreason=planted\n' "$2" "$3" "$4" "$5" >"$1/owner"
}
status_json() { # file, then name:liveness:state ...
  local f=$1 first=1 e; shift
  printf '{"agents":[' >"$f"
  for e in "$@"; do
    [ $first -eq 1 ] || printf ',' >>"$f"; first=0
    IFS=: read -r n lv st <<EOF
$e
EOF
    printf '{"name":"%s","liveness":"%s","state":"%s"}' "$n" "$lv" "$st" >>"$f"
  done
  printf ']}' >>"$f"
}
NOW=$(date +%s)
OLD=$((NOW - 100000))
unset FAKE_STATUS

echo "== host-lock.sh"

# H1: a free lock is taken; the owner file has agent, host, pid, started, reason
D="$T/h1"
"$L" take "$D" --holder a@x --reason "cargo test" --no-board 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^agent=a@x$' "$D/owner" && grep -q '^host=hostA$' "$D/owner" \
  && grep -q "^pid=$$\$" "$D/owner" && grep -q '^started=20' "$D/owner" && grep -q '^reason=cargo test$' "$D/owner" && ok=0
case_result "H1 free lock is taken, owner file complete" $ok "rc=$rc"
# the default pid is this shell, an ancestor of every call: from here on the holder is a foreign live pid
plant "$D" a@x hostA "$LIVE" "$NOW"

# H2: the same lock, live owner (pid = this shell): a second taker is refused, owner unchanged
"$L" take "$D" --holder b@x --no-board 2>"$T/h2.err"; rc=$?
ok=1; [ "$rc" -eq 75 ] && grep -q '^agent=a@x$' "$D/owner" && grep -q 'held by a@x' "$T/h2.err" && ok=0
case_result "H2 held live lock refuses the second taker (75)" $ok "rc=$rc"

# H2b: with --wait the second taker gets the lock once the first releases
( sleep 1; "$L" release "$D" --holder a@x 2>/dev/null ) &
"$L" take "$D" --holder b@x --no-board --wait 10 --poll 0.3 --pid $LIVE 2>/dev/null; rc=$?
wait
ok=1; [ "$rc" -eq 0 ] && grep -q '^agent=b@x$' "$D/owner" && ok=0
case_result "H2b --wait takes the lock after the owner releases" $ok "rc=$rc"
"$L" release "$D" --holder b@x 2>/dev/null

# H3: dead pid on this host -> taken over, ONE log line naming old owner and reason
D="$T/h3"; plant "$D" old@x hostA "$(dead_pid)" "$NOW"
"$L" take "$D" --holder new@x --no-board --pid $LIVE 2>"$T/h3.err"; rc=$?
n=$(grep -c 'took over stale lock' "$T/h3.err")
ok=1; [ "$rc" -eq 0 ] && [ "$n" -eq 1 ] && grep -q 'old owner old@x@hostA' "$T/h3.err" && grep -q 'is gone on host hostA' "$T/h3.err" \
  && grep -q '^agent=new@x$' "$D/owner" && [ ! -e "$D.takeover" ] && ok=0
case_result "H3 dead pid is taken over, one log line with owner and reason" $ok "rc=$rc, log lines=$n"
# H3b: a live pid of ANOTHER user (pid 1) is alive, although `kill -0` is refused
D="$T/h3b"; plant "$D" other@x hostA 1 "$NOW"
"$L" take "$D" --holder new@x --no-board --pid $LIVE 2>"$T/h3b.err"; rc=$?
s=$("$L" status "$D" --no-board 2>/dev/null)
ok=1; [ "$rc" -eq 75 ] && grep -q '^agent=other@x$' "$D/owner" && printf '%s' "$s" | grep -q 'alive=yes' && ok=0
case_result "H3b a live pid of another user is not stale (kill -0 EPERM)" $ok "rc=$rc, status=${s:0:80}"

# H4: too old (live pid, over --max-age) -> taken over; H4b the same lock but young -> refused
D="$T/h4"; plant "$D" old@x hostA $LIVE "$OLD"
"$L" take "$D" --holder new@x --no-board --max-age 3600 --pid $LIVE 2>"$T/h4.err"; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q 'old, over the limit of 3600' "$T/h4.err" && grep -q '^agent=new@x$' "$D/owner" && ok=0
case_result "H4 live pid but older than --max-age is taken over" $ok "rc=$rc"
D="$T/h4b"; plant "$D" old@x hostA $LIVE "$NOW"
"$L" take "$D" --holder new@x --no-board --max-age 3600 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 75 ] && grep -q '^agent=old@x$' "$D/owner" && ok=0
case_result "H4b live pid and young lock is NOT taken over" $ok "rc=$rc"

# H5: another host, dead-looking pid: never judged by pid -> refused. H5b the same, but old -> taken over
D="$T/h5"; plant "$D" far@y hostB "$(dead_pid)" "$NOW"
"$L" take "$D" --holder new@x --no-board 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 75 ] && grep -q '^agent=far@y$' "$D/owner" && ok=0
case_result "H5 other host: a dead pid number is not judged (refused)" $ok "rc=$rc"
D="$T/h5b"; plant "$D" far@y hostB "$(dead_pid)" "$OLD"
"$L" take "$D" --holder new@x --no-board --max-age 3600 --pid $LIVE 2>"$T/h5b.err"; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q 'old owner far@y@hostB' "$T/h5b.err" && grep -q 'old, over the limit' "$T/h5b.err" && ok=0
case_result "H5b other host: too old is taken over by the age limit" $ok "rc=$rc"

# H6: release by a non-owner is refused and the lock stays; wrong host too; the owner can release
D="$T/h6"; "$L" take "$D" --holder a@x --no-board 2>/dev/null
"$L" release "$D" --holder b@x 2>"$T/h6.err"; rc=$?
ok=1; [ "$rc" -eq 3 ] && [ -f "$D/owner" ] && grep -q 'refused' "$T/h6.err" && ok=0
case_result "H6 release by a non-owner is refused (3), lock stays" $ok "rc=$rc"
HOST_LOCK_HOST=hostC "$L" release "$D" --holder a@x 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 3 ] && [ -f "$D/owner" ] && ok=0
case_result "H6b release from another host is refused (3)" $ok "rc=$rc"
"$L" release "$D" --holder a@x 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ ! -e "$D" ] && ok=0
case_result "H6c release by the owner removes the lock" $ok "rc=$rc"

# H7: 8 concurrent takers on a free lock -> exactly one wins
D="$T/h7"; rm -rf "$T/h7.out"; mkdir -p "$T/h7.out"
i=1; while [ $i -le 8 ]; do
  ( "$L" take "$D" --holder "t$i@x" --no-board --pid $LIVE 2>/dev/null; echo $? >"$T/h7.out/$i" ) &
  i=$((i + 1))
done; wait
wins=$(cat "$T/h7.out"/* | grep -c '^0$'); busy=$(cat "$T/h7.out"/* | grep -c '^75$')
ok=1; [ "$wins" -eq 1 ] && [ "$busy" -eq 7 ] && ok=0
case_result "H7 8 concurrent takers on a free lock: exactly one wins" $ok "wins=$wins, refused=$busy"

# H7b: 8 concurrent takers on a STALE lock -> exactly one holder, exactly one takeover line
D="$T/h7b"; plant "$D" dead@x hostA "$(dead_pid)" "$NOW"; rm -rf "$T/h7b.out"; mkdir -p "$T/h7b.out"
i=1; while [ $i -le 8 ]; do
  ( "$L" take "$D" --holder "t$i@x" --no-board --pid $LIVE 2>"$T/h7b.out/$i.err"; echo $? >"$T/h7b.out/$i" ) &
  i=$((i + 1))
done; wait
wins=$(cat "$T/h7b.out"/? | grep -c '^0$')
lines=$(cat "$T/h7b.out"/*.err | grep -c 'took over stale lock')
holder=$(sed -n 's/^agent=//p' "$D/owner")
ok=1; [ "$wins" -eq 1 ] && [ "$lines" -eq 1 ] && [ -n "$holder" ] && [ ! -e "$D.takeover" ] && ok=0
case_result "H7b 8 takers on a stale lock: one holder, one takeover line" $ok "wins=$wins, takeover lines=$lines, holder=$holder"

# H8: the board (answered by the stub).
if :; then   # the board check runs in the binary: no jq
  export FAKE_STATUS="$T/status.json"
  status_json "$FAKE_STATUS" "own@x:dead:active" "alive@x:alive:active" "gone@x:-:left"
  D="$T/h8"; plant "$D" own@x hostA $LIVE "$NOW"
  "$L" take "$D" --holder new@x --pid $LIVE 2>"$T/h8.err"; rc=$?
  ok=1; [ "$rc" -eq 0 ] && grep -q 'agent own@x is dead' "$T/h8.err" && grep -q '^agent=new@x$' "$D/owner" && ok=0
  case_result "H8 owner agent dead on the board: taken over" $ok "rc=$rc"
  D="$T/h8b"; plant "$D" gone@x hostA $LIVE "$NOW"
  "$L" take "$D" --holder new@x --pid $LIVE 2>"$T/h8b.err"; rc=$?
  ok=1; [ "$rc" -eq 0 ] && grep -q 'agent gone@x has left' "$T/h8b.err" && ok=0
  case_result "H8b owner agent has left the board: taken over" $ok "rc=$rc"
  D="$T/h8c"; plant "$D" alive@x hostA $LIVE "$NOW"
  "$L" take "$D" --holder new@x 2>/dev/null; rc=$?
  ok=1; [ "$rc" -eq 75 ] && grep -q '^agent=alive@x$' "$D/owner" && ok=0
  case_result "H8c owner agent alive on the board: NOT taken over" $ok "rc=$rc"
  D="$T/h8d"; plant "$D" unknown@x hostA $LIVE "$NOW"
  "$L" take "$D" --holder new@x 2>/dev/null; rc=$?
  ok=1; [ "$rc" -eq 75 ] && ok=0
  case_result "H8d owner agent not listed on the board: not judged" $ok "rc=$rc"
  D="$T/h8e"; plant "$D" own@x hostA $LIVE "$NOW"
  "$L" take "$D" --holder new@x --no-board 2>/dev/null; rc=$?
  ok=1; [ "$rc" -eq 75 ] && ok=0
  case_result "H8e --no-board skips the board check" $ok "rc=$rc"
  rm -f "$FAKE_STATUS"
  D="$T/h8f"; plant "$D" own@x hostA $LIVE "$NOW"
  "$L" take "$D" --holder new@x 2>"$T/h8f.err"; rc=$?
  ok=1; [ "$rc" -eq 75 ] && grep -q 'board check skipped' "$T/h8f.err" && ok=0
  case_result "H8f board unreachable: skipped with a note, lock not stale" $ok "rc=$rc"
  # H8g: the 60 s cache works. Many polls of a wait give ONE `crewboard status` call.
  export FAKE_STATUS="$T/status.json" FAKE_CALLS="$T/h8g.calls"
  status_json "$FAKE_STATUS" "alive@x:alive:active"
  D="$T/h8g"; plant "$D" alive@x hostA $LIVE "$NOW"; : >"$FAKE_CALLS"
  "$L" take "$D" --holder new@x --wait 2 --poll 0.4 2>/dev/null; rc=$?
  calls=$(wc -l <"$FAKE_CALLS" | tr -d ' ')
  ok=1; [ "$rc" -eq 75 ] && [ "$calls" -eq 1 ] && ok=0
  case_result "H8g board cache: 5 or more polls, one status call" $ok "rc=$rc, status calls=$calls"
  # H8h: status fails 3 times, then works: one note with the stderr text, one "works again"
  D="$T/h8h"; plant "$D" alive@x hostA $LIVE "$NOW"; export FAKE_CALLS="$T/h8h.calls" FAKE_FAIL_FIRST=3; : >"$FAKE_CALLS"
  HOST_LOCK_BOARD_TTL=0 "$L" take "$D" --holder new@x --wait 3 --poll 0.3 2>"$T/h8h.err"; rc=$?
  skips=$(grep -c 'board check skipped' "$T/h8h.err"); again=$(grep -c 'board check works again' "$T/h8h.err")
  ok=1; [ "$rc" -eq 75 ] && [ "$skips" -eq 1 ] && [ "$again" -eq 1 ] && grep -q 'skipped: crewboard status failed: error: API rate limit exceeded$' "$T/h8h.err" \
    && ! grep -q 'second line of detail' "$T/h8h.err" && ok=0
  case_result "H8h 3 failures then ok: 1 skip note with cause, 1 works again" $ok "rc=$rc, skips=$skips, again=$again"
  # H8h2: a failing check does not stop the wait: the lock is still taken when the owner pid is gone
  D="$T/h8h2"; plant "$D" alive@x hostA "$(dead_pid)" "$NOW"; export FAKE_CALLS="$T/h8h2.calls" FAKE_FAIL_FIRST=99; : >"$FAKE_CALLS"
  "$L" take "$D" --holder new@x --pid $LIVE 2>"$T/h8h2.err"; rc=$?
  ok=1; [ "$rc" -eq 0 ] && grep -q 'reason=pid-dead' "$T/h8h2.err" && grep -q '^agent=new@x$' "$D/owner" && ok=0
  case_result "H8h2 board check failing: pid-dead takeover still works" $ok "rc=$rc"
  unset FAKE_FAIL_FIRST
  # H8i: a dead owner is taken over with board-dead, and the check asks the board once
  export FAKE_CALLS="$T/h8i.calls"; status_json "$FAKE_STATUS" "own@x:dead:active"
  D="$T/h8i"; plant "$D" own@x hostA $LIVE "$NOW"; : >"$FAKE_CALLS"
  "$L" take "$D" --holder new@x --pid $LIVE 2>"$T/h8i.err"; rc=$?
  calls=$(wc -l <"$FAKE_CALLS" | tr -d ' ')
  ok=1; [ "$rc" -eq 0 ] && grep -q 'reason=board-dead' "$T/h8i.err" && grep -q '^agent=new@x$' "$D/owner" && [ "$calls" -eq 1 ] && ok=0
  case_result "H8i dead owner: taken over as board-dead, one status call" $ok "rc=$rc, status calls=$calls"
  unset FAKE_STATUS FAKE_CALLS FAKE_FAIL_FIRST
fi

# H9: run form: holds the lock while CMD runs, releases after, passes the exit code on
D="$T/h9"
"$L" run "$D" --holder a@x --no-board -- bash -c '"$0" status "$1" >"$2"; exit 7' "$L" "$D" "$T/h9.status" 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 7 ] && grep -q '^held: a@x@hostA' "$T/h9.status" && [ ! -e "$D" ] && ok=0
case_result "H9 run holds the lock, releases it, returns the exit code" $ok "rc=$rc, during='$(cat "$T/h9.status")'"
D="$T/h9b"; plant "$D" live@x hostA $LIVE "$NOW"
"$L" run "$D" --holder a@x --no-board -- touch "$T/h9b.ran" 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 75 ] && [ ! -e "$T/h9b.ran" ] && grep -q '^agent=live@x$' "$D/owner" && ok=0
case_result "H9b run on a held live lock: CMD does not start (75)" $ok "rc=$rc"

# H10: status: free 1, held 0, stale 2
D="$T/h10"
out=$("$L" status "$D"); rc1=$?
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null
out2=$("$L" status "$D" --no-board); rc2=$?
rm -rf "$D"; plant "$D" old@x hostA "$(dead_pid)" "$NOW"
out3=$("$L" status "$D" --no-board); rc3=$?
ok=1; [ "$rc1" -eq 1 ] && [ "$out" = free ] && [ "$rc2" -eq 0 ] && [ "$rc3" -eq 2 ] \
  && case "$out2" in held:*) true ;; *) false ;; esac && case "$out3" in stale:*) true ;; *) false ;; esac && ok=0
case_result "H10 status: free=1, held=0, stale=2" $ok "rc=$rc1/$rc2/$rc3"

# H10b: status on a live lock says alive=yes and age=; a dead pid is stale (pid-dead)
D="$T/h10b"; plant "$D" a@hermes hostA "$LIVE" "$NOW"
out=$("$L" status "$D" --no-board); rc=$?
ok=1; [ "$rc" -eq 0 ] && case "$out" in "held: a@hermes@hostA pid="*" alive=yes age="[0-9]*) true ;; *) false ;; esac && ok=0
case_result "H10b status on a live lock: alive=yes and age=" $ok "rc=$rc, out=$out"
rm -rf "$D"; plant "$D" a@hermes hostA "$(dead_pid)" "$NOW"
out=$("$L" status "$D" --no-board); rc=$?
ok=1; [ "$rc" -eq 2 ] && case "$out" in "stale: pid-dead:"*) true ;; *) false ;; esac && ok=0
case_result "H10c status on a dead-pid lock: exit 2, pid-dead" $ok "rc=$rc, out=$out"
# H10d: break --if-dead on a live lock: refused (3), owner file unchanged
D="$T/h10d"; plant "$D" a@hermes hostA "$LIVE" "$NOW"; cp "$D/owner" "$T/h10d.before"
out=$("$L" break "$D" --if-dead --no-board 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 3 ] && case "$out" in "refused: held: a@hermes@hostA"*) true ;; *) false ;; esac && cmp -s "$D/owner" "$T/h10d.before" && ok=0
case_result "H10d break --if-dead on a live lock: exit 3, owner file unchanged" $ok "rc=$rc, out=$out"
# H10e: break --if-dead on a dead-pid lock: removed, exit 0, one log line
D="$T/h10e"; plant "$D" a@hermes hostA "$(dead_pid)" "$NOW"
"$L" break "$D" --if-dead --no-board --log "$T/h10e.log" 2>"$T/h10e.err"; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ ! -e "$D" ] && [ ! -e "$D.takeover" ] && grep -q 'took over stale lock' "$T/h10e.err" && [ "$(grep -c 'stale lock' "$T/h10e.log")" -eq 1 ] && ok=0
case_result "H10e break --if-dead on a dead-pid lock: removed, exit 0" $ok "rc=$rc"
# H10f: break --if-dead on a free dir prints free (0); H10g: without --if-dead it is a usage error (64) and the lock stays
out=$("$L" break "$T/h10f" --if-dead --no-board 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 0 ] && [ "$out" = free ] && ok=0
case_result "H10f break --if-dead on a free dir: free, exit 0" $ok "rc=$rc, out=$out"
D="$T/h10g"; plant "$D" a@hermes hostA "$(dead_pid)" "$NOW"
"$L" break "$D" --no-board 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 64 ] && [ -f "$D/owner" ] && ok=0
case_result "H10g break without --if-dead: exit 64, lock stays" $ok "rc=$rc"

# H11: a lock dir without an owner file: FRESH is never taken over, OLDER than the grace is
D="$T/h11b"; mkdir -p "$D"
"$L" take "$D" --holder new@x --no-board --pid $LIVE 2>"$T/h11b.err"; rc=$?
ok=1; [ "$rc" -eq 75 ] && [ -d "$D" ] && [ ! -e "$D/owner" ] && ! grep -q 'took over' "$T/h11b.err" && ok=0
case_result "H11 FRESH empty lock dir is NOT taken over" $ok "rc=$rc"
touch -t 202001010000 "$D"
"$L" take "$D" --holder new@x --no-board --pid $LIVE --empty-grace 30 2>"$T/h11.err"; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q 'reason=empty-after-grace' "$T/h11.err" && grep -q '^agent=new@x$' "$D/owner" && ok=0
case_result "H11b empty lock dir older than the grace: taken, empty-after-grace" $ok "rc=$rc"

# H12: every takeover log line of this run names a reason from the closed list
tot=0; named=0
for f in "$T"/h3.err "$T"/h4.err "$T"/h5b.err "$T"/h8.err "$T"/h8b.err "$T"/h11.err "$T"/h7b.out/*.err; do
  [ -f "$f" ] || continue
  c=$(grep -c 'took over stale lock' "$f"); tot=$((tot + c))
  c2=$(grep 'took over stale lock' "$f" | grep -c 'reason=\(pid-dead\|too-old\|board-dead\|board-left\|empty-after-grace\) ('); named=$((named + c2))
done
ok=1; [ "$tot" -ge 6 ] && [ "$tot" -eq "$named" ] && ok=0
case_result "H12 every takeover line names a reason from the list" $ok "takeovers=$tot, named=$named"

# H13: CI first. A running Runner.Worker keeps an AGENT from taking a free lock; without CI it takes
D="$T/h13"; echo "$RUNNER_LINE" >"$T/ps.txt"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>"$T/h13.err"; rc=$?
ok=1; [ "$rc" -eq 76 ] && [ ! -e "$D" ] && grep -q 'CI first' "$T/h13.err" && ok=0
case_result "H13 Runner.Worker runs: agent does not take a free lock (76)" $ok "rc=$rc"
: >"$T/ps.txt"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && [ -f "$D/owner" ] && ok=0
case_result "H13b no CI: the same take succeeds" $ok "rc=$rc"
"$L" release "$D" --holder a@x 2>/dev/null

# H13c: --wait: the agent waits while Runner.Worker runs and proceeds when it is gone
echo "$RUNNER_LINE" >"$T/ps.txt"
( sleep 1; : >"$T/ps.txt" ) &
"$L" take "$D" --holder a@x --no-board --pid $LIVE --wait 10 --poll 0.3 2>/dev/null; rc=$?
wait
ok=1; [ "$rc" -eq 0 ] && [ -f "$D/owner" ] && ok=0
case_result "H13c agent waits for Runner.Worker, then takes the lock" $ok "rc=$rc"
"$L" release "$D" --holder a@x 2>/dev/null

# H14: waiting list. ci-wait registers; an agent is refused (76); ci-done removes; then it takes
D="$T/h14"
"$L" ci-wait "$D" --holder ci-job-1 2>/dev/null; rc0=$?
[ -f "$D.waiting/ci-job-1" ] && have=1 || have=0
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc=$?
ok=1; [ "$rc0" -eq 0 ] && [ "$have" -eq 1 ] && [ "$rc" -eq 76 ] && [ ! -e "$D" ] && ok=0
case_result "H14 waiting-list entry: agent refused (76)" $ok "ci-wait rc=$rc0, take rc=$rc"
"$L" ci-status "$D"; rs=$?
"$L" ci-done "$D" --holder ci-job-1 2>/dev/null
"$L" ci-status "$D" >/dev/null; rs2=$?
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc=$?
ok=1; [ "$rs" -eq 0 ] && [ "$rs2" -eq 1 ] && [ "$rc" -eq 0 ] && [ ! -e "$D.waiting" ] && ok=0
case_result "H14b ci-done: ci-status 0 then 1, agent takes the lock" $ok "ci-status=$rs/$rs2, take rc=$rc"
"$L" release "$D" --holder a@x 2>/dev/null
# H14c: an entry older than --ci-max-age is ignored
"$L" ci-wait "$D" --holder ci-old 2>/dev/null; touch -t 202001010000 "$D.waiting/ci-old"
"$L" take "$D" --holder a@x --no-board --pid $LIVE --ci-max-age 3600 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && ok=0
case_result "H14c old waiting-list entry is ignored" $ok "rc=$rc"
"$L" release "$D" --holder a@x 2>/dev/null; "$L" ci-done "$D" --holder ci-old

# H15: a CI take proceeds past a waiting entry and a running Runner.Worker; a HELD lock still blocks it
D="$T/h15"; echo "$RUNNER_LINE" >"$T/ps.txt"; "$L" ci-wait "$D" --holder other-ci 2>/dev/null
"$L" take "$D" --holder ci-1 --ci --no-board --pid $LIVE 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^agent=ci-1$' "$D/owner" && [ ! -e "$D.waiting/ci-1" ] && ok=0
case_result "H15 CI take proceeds past waiting CI and Runner.Worker" $ok "rc=$rc"
"$L" take "$D" --holder ci-2 --ci --no-board --pid $LIVE --wait 1 --poll 0.3 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 75 ] && grep -q '^agent=ci-1$' "$D/owner" && [ ! -e "$D.waiting/ci-2" ] && ok=0
case_result "H15b CI take is blocked by a HELD lock (75), entry removed" $ok "rc=$rc"
( "$L" take "$D" --holder ci-3 --ci --no-board --pid $LIVE --wait 10 --poll 0.3 2>/dev/null; echo $? >"$T/h15.rc" ) &
sleep 0.7; [ -f "$D.waiting/ci-3" ] && during=1 || during=0
"$L" release "$D" --holder ci-1 2>/dev/null; wait
ok=1; [ "$during" -eq 1 ] && [ "$(cat "$T/h15.rc")" -eq 0 ] && grep -q '^agent=ci-3$' "$D/owner" && ok=0
case_result "H15c waiting CI take is on the list while it waits, then gets the lock" $ok "listed=$during, rc=$(cat "$T/h15.rc")"
"$L" release "$D" --holder ci-3 2>/dev/null; "$L" ci-done "$D" --holder other-ci; : >"$T/ps.txt"

# H16: an agent does not take a STALE lock over while CI waits (lock unchanged); without CI it does
D="$T/h16"; plant "$D" dead@x hostA "$(dead_pid)" "$NOW"; "$L" ci-wait "$D" --holder ci-w 2>/dev/null
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 76 ] && grep -q '^agent=dead@x$' "$D/owner" && ok=0
case_result "H16 no takeover of a stale lock while CI waits (76)" $ok "rc=$rc"
"$L" ci-done "$D" --holder ci-w
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^agent=a@x$' "$D/owner" && ok=0
case_result "H16b CI gone: the stale lock is taken over" $ok "rc=$rc"
"$L" release "$D" --holder a@x 2>/dev/null

# H17: --pid runner. A fake Runner.Worker (a symlink to bash with that name) is the
# parent of the host-lock call. The walk is real: ps -o ppid= / comm= on this OS.
mkdir -p "$T/rw"; ln -s /bin/bash "$T/rw/Runner.Worker"
D="$T/h17"; : >"$T/ps.txt"
"$T/rw/Runner.Worker" -c '"$1" take "$2" --holder ci-r --ci --no-board --pid runner; echo $? >"$3"; sleep 60' rw "$L" "$D" "$T/h17.rc" 2>/dev/null &
RW=$!
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do [ -s "$T/h17.rc" ] && break; sleep 0.3; done
ok=1; [ "$(cat "$T/h17.rc" 2>/dev/null)" = 0 ] && grep -q "^pid=$RW\$" "$D/owner" && ok=0
case_result "H17 --pid runner records the Runner.Worker pid" $ok "rw=$RW, owner pid=$(sed -n 's/^pid=//p' "$D/owner" 2>/dev/null)"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 75 ] && grep -q '^agent=ci-r$' "$D/owner" && ok=0
case_result "H17b Runner.Worker alive: an agent does not take the lock (75)" $ok "rc=$rc"
kill "$RW" 2>/dev/null; wait "$RW" 2>/dev/null
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>"$T/h17c.err"; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^agent=a@x$' "$D/owner" && grep -q 'reason=pid-dead' "$T/h17c.err" && ok=0
case_result "H17c Runner.Worker killed: the agent takes over (pid-dead)" $ok "rc=$rc"
"$L" release "$D" --holder a@x 2>/dev/null

# H17d: the walk with a fake process table. Chain: script -> shell 500 -> Runner.Worker 400 -> init.
# macOS shows a full path as comm; the basename counts.
cat >"$T/pt.sh" <<'PT'
#!/bin/sh
case "$1" in
  500) echo "400 /bin/bash" ;;
  400) echo "300 /Users/x/actions-runner/bin/Runner.Worker" ;;
  300) echo "1 /sbin/launchd" ;;
  *)   echo "500 /bin/bash" ;;
esac
PT
chmod +x "$T/pt.sh"
D="$T/h17d"
HOST_LOCK_PARENT_CMD="$T/pt.sh \$1" "$L" take "$D" --holder ci-h --no-board --pid runner 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^pid=400$' "$D/owner" && ok=0
case_result "H17d walk finds Runner.Worker behind a full-path comm" $ok "rc=$rc, pid=$(sed -n 's/^pid=//p' "$D/owner" 2>/dev/null)"
"$L" release "$D" --holder ci-h 2>/dev/null

# H18: no Runner.Worker ancestor: exit 64, nothing written (both take and ci-wait)
cat >"$T/pt2.sh" <<'PT'
#!/bin/sh
case "$1" in
  500) echo "300 /bin/bash" ;;
  300) echo "1 /sbin/launchd" ;;
  *)   echo "500 /bin/bash" ;;
esac
PT
chmod +x "$T/pt2.sh"
D="$T/h18"
HOST_LOCK_PARENT_CMD="$T/pt2.sh \$1" "$L" take "$D" --holder ci-n --no-board --pid runner 2>"$T/h18.err"; rc1=$?
HOST_LOCK_PARENT_CMD="$T/pt2.sh \$1" "$L" ci-wait "$D" --holder ci-n --pid runner 2>/dev/null; rc2=$?
ok=1; [ "$rc1" -eq 64 ] && [ "$rc2" -eq 64 ] && [ ! -e "$D" ] && [ ! -e "$D.waiting" ] && grep -q 'no Runner.Worker' "$T/h18.err" && ok=0
case_result "H18 --pid runner without a Runner.Worker ancestor: 64, nothing written" $ok "rc=$rc1/$rc2"
"$L" take "$D" --holder ci-n --no-board --pid 0 --expect 10m 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^pid=0$' "$D/owner" && ok=0
case_result "H18b --pid 0 still works as before" $ok "rc=$rc"
"$L" release "$D" --holder ci-n 2>/dev/null

# R1 to R9: re-entrancy. The nesting is real processes: run in run, take under run.
# The inner scripts write their results to files, because the outer command owns the lock.
re_hold() { grep -q "^agent=$2\$" "$1/owner" 2>/dev/null && echo yes || echo no; }
D="$T/h19"; R="$T/h19"
"$L" run "$D" --holder outer@x --no-board -- bash -c '
  "$1" run "$2" --holder inner@x --no-board --wait 0 -- bash -c "echo inner-ran >\"$3.ran\""; echo $? >"$3.rc"
  grep -q "^agent=outer@x\$" "$2/owner" && echo kept >"$3.kept"
  [ "$(ls -d "$2".* 2>/dev/null | wc -l)" -eq 0 ] && echo nospare >"$3.nospare"
' h "$L" "$D" "$R" 2>"$T/h19.err"
ok=1; [ "$(cat "$R.rc" 2>/dev/null)" = 0 ] && [ -f "$R.ran" ] && [ "$(cat "$R.kept" 2>/dev/null)" = kept ] && [ ! -d "$D" ] \
  && grep -q 're-entrant' "$T/h19.err" && ok=0
case_result "R1 nested run in run passes at once; the outer lock stays until the outer ends" $ok "rc=$(cat "$R.rc" 2>/dev/null) kept=$(cat "$R.kept" 2>/dev/null) gone=$([ -d "$D" ] && echo no || echo yes)"

D="$T/h20"; R="$T/h20"
"$L" run "$D" --holder outer@x --no-board -- bash -c '
  "$1" take "$2" --holder tool@x --no-board --wait 0; echo $? >"$3.take"
  [ "$(grep -c "" "$2/owner")" -eq 6 ] && grep -q "^agent=outer@x\$" "$2/owner" && echo oneowner >"$3.one"
  "$1" release "$2" --holder tool@x; echo $? >"$3.rel"
  [ -f "$2/owner" ] && grep -q "^agent=outer@x\$" "$2/owner" && echo stays >"$3.stays"
  "$1" release "$2" --holder outer@x; echo $? >"$3.rel2"
  [ -f "$2/owner" ] && echo stays2 >"$3.stays2"
' h "$L" "$D" "$R" 2>/dev/null
ok=1; [ "$(cat "$R.take")" = 0 ] && [ "$(cat "$R.one" 2>/dev/null)" = oneowner ] && [ "$(cat "$R.rel")" = 0 ] && [ "$(cat "$R.stays" 2>/dev/null)" = stays ] \
  && [ "$(cat "$R.rel2")" = 0 ] && [ "$(cat "$R.stays2" 2>/dev/null)" = stays2 ] && [ ! -d "$D" ] && ok=0
case_result "R2 nested take under run: no second owner file; its release (any name) leaves the outer lock" $ok "take=$(cat "$R.take") rel=$(cat "$R.rel") stays=$(cat "$R.stays" 2>/dev/null)"

# R3: a holder that is NOT an ancestor still blocks (75), even if the environment names its pid
D="$T/h21"; sleep 30 & NA=$!
plant "$D" other@x hostA "$NA" "$NOW"
CREWBOARD_HOST_LOCK="$D" CREWBOARD_HOST_LOCK_PID="$NA" "$L" take "$D" --holder me@x --no-board --pid 0 --expect 10m 2>/dev/null; rc1=$?
CREWBOARD_HOST_LOCK="$D" CREWBOARD_HOST_LOCK_PID="$NA" "$L" release "$D" --holder me@x 2>/dev/null; rc2=$?
ok=1; [ "$rc1" -eq 75 ] && [ "$rc2" -eq 3 ] && grep -q '^agent=other@x$' "$D/owner" && ok=0
case_result "R3 a non-ancestor holder still blocks take (75) and release (3), env or not" $ok "take=$rc1 release=$rc2"
kill "$NA" 2>/dev/null; wait "$NA" 2>/dev/null

# R4: the holder is an ancestor, but the process is not granted: 77 at once, no wait
D="$T/h22"; R="$T/h22"
bash -c '"$1" take "$2" --holder a@x --no-board --pid $$ || exit 1
  s=$(date +%s); "$1" take "$2" --holder b@x --no-board --wait 30 --poll 1 2>"$3.err"; echo $? >"$3.rc"; echo $(( $(date +%s) - s )) >"$3.secs"
  "$1" run "$2" --holder b@x --no-board --wait 30 -- true 2>/dev/null; echo $? >"$3.rcrun"
  "$1" release "$2" --holder a@x 2>/dev/null; echo $? >"$3.rcrel"' h "$L" "$D" "$R"
ok=1; [ "$(cat "$R.rc")" = 77 ] && [ "$(cat "$R.rcrun")" = 77 ] && [ "$(cat "$R.secs")" -le 5 ] && grep -q 'held by your own parent process' "$R.err" && ok=0
case_result "R4 ancestor holder without a grant: 77 at once (take and run), message names the parent" $ok "take=$(cat "$R.rc") run=$(cat "$R.rcrun") secs=$(cat "$R.secs")"
# direction 2: in the same shell, a child releases the lock that its shell took with --pid: it frees it (no grant, no nesting)
rc=$(cat "$R.rcrel")
ok=1; [ "$rc" -eq 0 ] && [ ! -d "$D" ] && ok=0
case_result "R4b release by the owner under an ancestor pid without a grant still frees the lock" $ok "rc=$rc"

# R5: the child of run gets both variables; nothing is set outside
D="$T/h23"; R="$T/h23"
unset CREWBOARD_HOST_LOCK CREWBOARD_HOST_LOCK_PID
"$L" run "$D" --holder o@x --no-board -- bash -c 'echo "$CREWBOARD_HOST_LOCK" >"$1.dir"; echo "$CREWBOARD_HOST_LOCK_PID" >"$1.pid"; sed -n "s/^pid=//p" "$2/owner" >"$1.owner"' h "$R" "$D" 2>/dev/null
RD=$(cd "$T" && pwd -P)/h23
ok=1; [ "$(cat "$R.dir")" = "$RD" ] && [ -n "$(cat "$R.pid")" ] && [ "$(cat "$R.pid")" = "$(cat "$R.owner")" ] && [ -z "${CREWBOARD_HOST_LOCK+x}${CREWBOARD_HOST_LOCK_PID+x}" ] && ok=0
case_result "R5 run exports CREWBOARD_HOST_LOCK (resolved dir) and _PID (holder) to the child, not outside" $ok "dir=$(cat "$R.dir") pid=$(cat "$R.pid")"
"$L" take "$D" --holder o@x --no-board --pid 0 --expect 10m 2>/dev/null
env | grep -q '^CREWBOARD_HOST_LOCK' && ok=1 || ok=0
case_result "R5b take sets no variable in its caller" $ok ""
"$L" release "$D" --holder o@x 2>/dev/null

# R6: the dir is compared after resolving it: a symlink spelling and a ../ spelling are the same lock
D="$T/h24"; R="$T/h24"; ln -s "$D" "$T/h24.lnk"; mkdir -p "$T/sub"
"$L" run "$D" --holder o@x --no-board -- bash -c '
  "$1" take "$2" --holder t@x --no-board --wait 0; echo $? >"$4.a"
  "$1" take "$3/../h24" --holder t@x --no-board --wait 0; echo $? >"$4.b"
  "$1" release "$2" --holder t@x; "$1" release "$3/../h24" --holder t@x
  grep -q "^agent=o@x\$" "$5/owner" && echo stays >"$4.c"
' h "$L" "$T/h24.lnk" "$T/sub" "$R" "$D" 2>/dev/null
ok=1; [ "$(cat "$R.a")" = 0 ] && [ "$(cat "$R.b")" = 0 ] && [ "$(cat "$R.c" 2>/dev/null)" = stays ] && ok=0
case_result "R6 symlink and ../ spellings of the dir are the same lock (re-entry and nested release)" $ok "a=$(cat "$R.a") b=$(cat "$R.b") c=$(cat "$R.c" 2>/dev/null)"

# R7: the grant is for ONE dir: another dir is a different lock and blocks (75)
D="$T/h25"; D2="$T/h25b"; R="$T/h25"
"$L" run "$D" --holder o@x --no-board -- bash -c '
  "$1" run "$3" --holder x@x --no-board -- bash -c "\"\$1\" take \"\$2\" --holder y@x --no-board --wait 0 2>/dev/null; echo \$? >\"\$3\"" h "$1" "$3" "$4.rc"
' h "$L" "$D" "$D2" "$R" 2>/dev/null
ok=1; [ "$(cat "$R.rc")" = 0 ] && ok=0
case_result "R7 run in run on two dirs: the inner child is granted its own dir (take under it passes)" $ok "rc=$(cat "$R.rc" 2>/dev/null)"
"$L" run "$D" --holder o@x --no-board -- bash -c '
  mkdir -p "$2"; printf "agent=z@x\nhost=hostA\npid=$CREWBOARD_HOST_LOCK_PID\nstarted=p\nstarted_epoch=$(date +%s)\nreason=p\n" >"$2/owner"
  "$1" take "$2" --holder y@x --no-board --wait 0 2>/dev/null; echo $? >"$3.rc2"' h "$L" "$D2" "$R" 2>/dev/null
ok=1; [ "$(cat "$R.rc2")" = 77 ] && ok=0
case_result "R7b a lock of a DIFFERENT dir held by an ancestor is not covered by the outer grant (77)" $ok "rc=$(cat "$R.rc2" 2>/dev/null)"
rm -rf "$D2"

# H19: a ci-wait entry with a recorded pid. Dead pid: ignored and removed. Live pid: blocks (76).
D="$T/h19"; DP=$(dead_pid)
"$L" ci-wait "$D" --holder ci-dead --pid "$DP" 2>/dev/null
ok=1; grep -q "^pid=$DP\$" "$D.waiting/ci-dead" && ok=0
case_result "H19 ci-wait records the pid in the entry" $ok "$(tr '\n' ' ' <"$D.waiting/ci-dead")"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^agent=a@x$' "$D/owner" && [ ! -e "$D.waiting/ci-dead" ] && ok=0
case_result "H19b entry with a dead pid does not block an agent, is removed" $ok "rc=$rc"
"$L" release "$D" --holder a@x 2>/dev/null; "$L" ci-done "$D" --holder ci-dead
"$L" ci-wait "$D" --holder ci-live --pid $LIVE 2>/dev/null
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 76 ] && [ ! -e "$D" ] && [ -f "$D.waiting/ci-live" ] && ok=0
case_result "H19c entry with a live pid blocks an agent (76)" $ok "rc=$rc"
"$L" ci-done "$D" --holder ci-live
# an entry from another host with a dead pid is not judged; an old entry without pid behaves as before
D="$T/h19d"; mkdir -p "$D.waiting"
printf 'name=ci-far\nhost=hostB\nstarted=x\npid=%s\n' "$DP" >"$D.waiting/ci-far"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc1=$?
rm -f "$D.waiting/ci-far"
printf 'name=ci-old\nhost=hostA\nstarted=x\n' >"$D.waiting/ci-old"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc2=$?
ok=1; [ "$rc1" -eq 76 ] && [ "$rc2" -eq 76 ] && ok=0
case_result "H19d foreign-host pid and old entry without pid still block (76)" $ok "rc=$rc1/$rc2"
rm -rf "$D.waiting"

# F: a foreign holder: a lock dir with a file of a tool, but no owner file.
# F1: a truly EMPTY dir older than the grace is still stale (unchanged); F1b a foreign file is not.
D="$T/f1"; mkdir -p "$D"; touch -t 202001010000 "$D"
out=$("$L" status "$D" --no-board 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 2 ] && case "$out" in "stale: empty-after-grace"*) true ;; *) false ;; esac && ok=0
case_result "F1 empty dir older than the grace: stale, empty-after-grace" $ok "rc=$rc"
D="$T/f1b"; mkdir -p "$D"; echo "github-actions e2e-arm64 run 4711" >"$D/info"; touch -t 202001010000 "$D"
out=$("$L" status "$D" --no-board --max-age 0 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 0 ] && case "$out" in "held: foreign holder: info: github-actions e2e-arm64 run 4711") true ;; *) false ;; esac && ok=0
case_result "F1b old dir with an info file is held, not empty-after-grace" $ok "rc=$rc out=$out"
# F2: an info file, older than the grace but younger than --max-age: a take is refused (75), nothing is taken over
D="$T/f2"; mkdir -p "$D"; echo "github-actions e2e-arm64 run 4711" >"$D/info"; sleep 2
"$L" take "$D" --holder a@x --no-board --pid $LIVE --empty-grace 1 2>"$T/f2.err"; rc=$?
out=$("$L" status "$D" --no-board --empty-grace 1 2>/dev/null); rc2=$?
ok=1; [ "$rc" -eq 75 ] && [ "$rc2" -eq 0 ] && [ -f "$D/info" ] && [ ! -e "$D/owner" ] && ! grep -q 'took over' "$T/f2.err" && ok=0
case_result "F2 foreign info file older than the grace blocks a take (75)" $ok "rc=$rc status=$rc2"
# F3: the same dir older than --max-age is stale (too-old) and is taken over; --max-age 0 never
"$L" take "$D" --holder a@x --no-board --pid $LIVE --empty-grace 1 --max-age 0 2>/dev/null; rc0=$?
out=$("$L" status "$D" --no-board --empty-grace 1 --max-age 1 2>/dev/null); rcs=$?
"$L" take "$D" --holder a@x --no-board --pid $LIVE --empty-grace 1 --max-age 1 2>"$T/f3.err"; rc=$?
ok=1; [ "$rc0" -eq 75 ] && [ "$rcs" -eq 2 ] && [ "$rc" -eq 0 ] && grep -q 'reason=too-old' "$T/f3.err" \
  && grep -q 'old owner foreign holder: info: github-actions e2e-arm64 run 4711' "$T/f3.err" && grep -q '^agent=a@x$' "$D/owner" && [ ! -e "$D/info" ] && ok=0
case_result "F3 foreign dir older than --max-age is stale (too-old) and taken; --max-age 0 never" $ok "rc=$rc0/$rcs/$rc"
# F4: status, the wait message and the takeover line name the file and its first line; control characters
# are removed and the line is capped
D="$T/f4"; mkdir -p "$D"
{ printf 'job \033[31mred\ttab'; printf 'x%.0s' $(seq 1 300); printf '\nsecond line\n'; } >"$D/zz-info"
echo "aaa" >"$D/zz-later"
out=$("$L" status "$D" --no-board 2>/dev/null)
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>"$T/f4.err"
first=${out#held: foreign holder: zz-info: }
ok=1; case "$out" in "held: foreign holder: zz-info: job "*) true ;; *) false ;; esac && [ "${#first}" -le 120 ] && [ "${#first}" -ge 100 ] \
  && ! printf '%s' "$out" | LC_ALL=C grep -q "$(printf '[\001-\037\177]')" && ! printf '%s' "$out" | grep -q 'second line' \
  && grep -q 'held by foreign holder: zz-info: job' "$T/f4.err" && ok=0
case_result "F4 status and wait message name file and first line (capped, no control chars)" $ok "len=${#first}"
# F5: a recorded pid: dead = stale (pid-dead), live = blocks; another host's pid is not judged; host=this host is
D="$T/f5"; mkdir -p "$D"; printf 'tool x\npid=%s\n' "$(dead_pid)" >"$D/lockinfo"
out=$("$L" status "$D" --no-board 2>/dev/null); rcs=$?
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>"$T/f5.err"; rc=$?
ok=1; [ "$rcs" -eq 2 ] && [ "$rc" -eq 0 ] && grep -q 'reason=pid-dead' "$T/f5.err" && grep -q 'foreign holder: lockinfo: tool x' "$T/f5.err" && ok=0
case_result "F5 foreign dir with a DEAD recorded pid: stale (pid-dead), taken" $ok "status=$rcs take=$rc"
D="$T/f5b"; mkdir -p "$D"; printf 'tool x\npid=%s\n' "$LIVE" >"$D/lockinfo"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 75 ] && [ -f "$D/lockinfo" ] && [ ! -e "$D/owner" ] && ok=0
case_result "F5b foreign dir with a LIVE recorded pid blocks (75)" $ok "rc=$rc"
D="$T/f5c"; mkdir -p "$D"; printf 'tool x\nhost=hostB\npid=%s\n' "$(dead_pid)" >"$D/lockinfo"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc1=$?
printf 'tool x\nhost=hostA\npid=%s\n' "$(dead_pid)" >"$D/lockinfo"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc2=$?
ok=1; [ "$rc1" -eq 75 ] && [ "$rc2" -eq 0 ] && ok=0
case_result "F5c pid of another host not judged (75); same host dead pid taken" $ok "rc=$rc1/$rc2"
# F6: a taker's temp file is not a foreign file: a fresh dir with only owner.tmp.* is still not taken over,
# and an old one is empty-after-grace as before. release by a stranger leaves a foreign dir alone (3)
D="$T/f6"; mkdir -p "$D"; echo "agent=x" >"$D/owner.tmp.123"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>/dev/null; rc1=$?
touch -t 202001010000 "$D"
"$L" take "$D" --holder a@x --no-board --pid $LIVE 2>"$T/f6.err"; rc2=$?
ok=1; [ "$rc1" -eq 75 ] && [ "$rc2" -eq 0 ] && grep -q 'reason=empty-after-grace' "$T/f6.err" && ok=0
case_result "F6 owner.tmp.* only: fresh is kept (75), old is empty-after-grace" $ok "rc=$rc1/$rc2"
D="$T/f6b"; mkdir -p "$D"; echo "i" >"$D/info"
"$L" release "$D" --holder a@x 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 3 ] && [ -f "$D/info" ] && ok=0
case_result "F6b release of a foreign dir is refused (3), its file stays" $ok "rc=$rc"

# O: an invalid owner file. An `owner` file without a non-empty agent= or without a
# numeric started_epoch= is judged as a foreign holder: a pid= line in it, or too-old counted
# from the mtime of the owner file.
# O1: a one-line owner file older than --max-age (by its own mtime; the dir is fresh): stale, taken, named
D="$T/o1"; mkdir -p "$D"; echo 'claude@host-a' >"$D/owner"; touch -t 202001010000 "$D/owner"
out=$("$L" status "$D" --no-board --max-age 3600 2>/dev/null); rcs=$?
"$L" take "$D" --holder new@x --no-board --pid $LIVE --max-age 3600 2>"$T/o1.err"; rc=$?
ok=1; [ "$rcs" -eq 2 ] && case "$out" in "stale: too-old: the invalid owner file is "*" s old, over the limit of 3600 s; owner foreign holder: owner (no key=value fields): claude@host-a") true ;; *) false ;; esac \
  && [ "$rc" -eq 0 ] && grep -q 'old owner foreign holder: owner (no key=value fields): claude@host-a; reason=too-old (the invalid owner file is' "$T/o1.err" \
  && [ "$(grep -c 'took over stale lock' "$T/o1.err")" -eq 1 ] && grep -q '^agent=new@x$' "$D/owner" && ok=0
case_result "O1 one-line owner file older than --max-age (file mtime): too-old, taken" $ok "status=$rcs take=$rc out=$out"
"$L" release "$D" --holder new@x 2>/dev/null
# O1b: the same old one-line owner file with --max-age 0: never too-old, held (75)
D="$T/o1b"; mkdir -p "$D"; echo 'claude@host-a' >"$D/owner"; touch -t 202001010000 "$D/owner"
"$L" take "$D" --holder new@x --no-board --pid $LIVE --max-age 0 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 75 ] && [ "$(/bin/cat "$D/owner")" = 'claude@host-a' ] && ok=0
case_result "O1b old one-line owner file, --max-age 0: held (75), file unchanged" $ok "rc=$rc"
# O2: a FRESH one-line owner file in an OLD dir: held; status and the wait message name it (not "@ pid= since")
D="$T/o2"; mkdir -p "$D"; echo 'claude@host-a' >"$D/owner"; touch -t 202001010000 "$D"
out=$("$L" status "$D" --no-board 2>/dev/null); rcs=$?
# --wait 2, as E4b: with --wait 1, a first try that crosses a second boundary ends the wait
# at once (date +%s), and only the final "is held by" line is printed, not the wait message
"$L" take "$D" --holder new@x --no-board --pid $LIVE --wait 2 --poll 0.3 2>"$T/o2.err"; rc=$?
ok=1; [ "$rcs" -eq 0 ] && [ "$out" = 'held: foreign holder: owner (no key=value fields): claude@host-a' ] \
  && [ "$rc" -eq 75 ] && grep -q 'waiting for lock .* held by foreign holder: owner (no key=value fields): claude@host-a' "$T/o2.err" \
  && ! grep -q 'took over' "$T/o2.err" && [ "$(/bin/cat "$D/owner")" = 'claude@host-a' ] && ok=0
case_result "O2 fresh one-line owner file (old dir): held (status 0, take 75), named" $ok "status=$rcs take=$rc out=$out err=$(tr '\n' '~' <"$T/o2.err" | cut -c1-300)"
# O3: key=value lines but no started_epoch=: a dead pid= line in it is pid-dead; status names the problem
D="$T/o3"; mkdir -p "$D"; printf 'agent=a@x\nhost=hostA\npid=%s\n' "$(dead_pid)" >"$D/owner"
out=$("$L" status "$D" --no-board 2>/dev/null); rcs=$?
"$L" take "$D" --holder new@x --no-board --pid $LIVE 2>"$T/o3.err"; rc=$?
ok=1; [ "$rcs" -eq 2 ] && case "$out" in "stale: pid-dead: pid "*" recorded in owner is gone; owner foreign holder: owner (no numeric started_epoch=): agent=a@x") true ;; *) false ;; esac \
  && [ "$rc" -eq 0 ] && grep -q 'reason=pid-dead' "$T/o3.err" && grep -q '^agent=new@x$' "$D/owner" && ok=0
case_result "O3 owner file without started_epoch=, dead pid: pid-dead, taken" $ok "status=$rcs take=$rc out=$out"
"$L" release "$D" --holder new@x 2>/dev/null
# O3b: the same with a LIVE pid, or a dead pid of another host: held (75); the problem text for agent= too
D="$T/o3b"; mkdir -p "$D"; printf 'agent=a@x\nhost=hostA\npid=%s\n' "$LIVE" >"$D/owner"
out1=$("$L" status "$D" --no-board 2>/dev/null); rc1=$?
"$L" take "$D" --holder new@x --no-board --pid $LIVE 2>/dev/null; rc2=$?
printf 'agent=a@x\nhost=hostB\npid=%s\n' "$(dead_pid)" >"$D/owner"
"$L" take "$D" --holder new@x --no-board --pid $LIVE 2>/dev/null; rc3=$?
printf 'agent=\nstarted_epoch=%s\n' "$NOW" >"$D/owner"
out4=$("$L" status "$D" --no-board 2>/dev/null)
printf 'started_epoch=yesterday\n' >"$D/owner"
out5=$("$L" status "$D" --no-board 2>/dev/null)
ok=1; [ "$rc1" -eq 0 ] && [ "$out1" = 'held: foreign holder: owner (no numeric started_epoch=): agent=a@x' ] && [ "$rc2" -eq 75 ] && [ "$rc3" -eq 75 ] \
  && [ "$out4" = 'held: foreign holder: owner (no agent= value): agent=' ] \
  && [ "$out5" = 'held: foreign holder: owner (no agent= value, no numeric started_epoch=): started_epoch=yesterday' ] && ok=0
case_result "O3b invalid owner file, live or foreign-host pid: held (75); problem named" $ok "rc=$rc1/$rc2/$rc3 out4=$out4 out5=$out5"
# O4: release by the agent and host that an invalid owner file names is refused (3); once valid, it releases
D="$T/o4"; mkdir -p "$D"; printf 'agent=a@x\nhost=hostA\npid=%s\nstarted=x\n' "$LIVE" >"$D/owner"
"$L" release "$D" --holder a@x 2>"$T/o4.err"; rc1=$?
[ -f "$D/owner" ] && kept=1 || kept=0
printf 'started_epoch=%s\n' "$NOW" >>"$D/owner"
"$L" release "$D" --holder a@x 2>/dev/null; rc2=$?
ok=1; [ "$rc1" -eq 3 ] && [ "$kept" -eq 1 ] && grep -q 'refused: .* held by foreign holder: owner (no numeric started_epoch=)' "$T/o4.err" \
  && [ "$rc2" -eq 0 ] && [ ! -e "$D" ] && ok=0
case_result "O4 release of an invalid owner file is refused (3); valid, it is released" $ok "rc=$rc1/$rc2"
# O5: a VALID owner file keeps its rules: an old file mtime does not make it too-old (started_epoch counts)
D="$T/o5"; plant "$D" a@x hostA "$LIVE" "$NOW"; touch -t 202001010000 "$D/owner" "$D"
out=$("$L" status "$D" --no-board --max-age 3600 2>/dev/null); rcs=$?
"$L" take "$D" --holder new@x --no-board --pid $LIVE --max-age 3600 2>/dev/null; rc=$?
ok=1; [ "$rcs" -eq 0 ] && case "$out" in "held: a@x@hostA pid=$LIVE since planted (planted) alive=yes age="*) true ;; *) false ;; esac && [ "$rc" -eq 75 ] && ok=0
case_result "O5 valid owner file with an old mtime: held by started_epoch (75)" $ok "status=$rcs take=$rc out=$out"
# O6: an `owner` that is a directory: not a regular file; fresh is held, old (its mtime) is too-old
D="$T/o6"; mkdir -p "$D/owner"
out1=$("$L" status "$D" --no-board 2>/dev/null); rc1=$?
touch -t 202001010000 "$D/owner"
"$L" take "$D" --holder new@x --no-board --pid $LIVE 2>"$T/o6.err"; rc2=$?
ok=1; [ "$rc1" -eq 0 ] && [ "$out1" = 'held: foreign holder: owner (not a regular file)' ] && [ "$rc2" -eq 0 ] \
  && grep -q 'old owner foreign holder: owner (not a regular file); reason=too-old' "$T/o6.err" && grep -q '^agent=new@x$' "$D/owner" && ok=0
case_result "O6 owner is a directory: fresh held, old taken (too-old)" $ok "rc=$rc1/$rc2 out=$out1"
"$L" release "$D" --holder new@x 2>/dev/null

echo
echo "== gate-loop.sh (shim for crewboard repeat)"
R="$T/repo"; mkdir -p "$R"
git -C "$R" init -q 2>/dev/null
git -C "$R" -c user.email=t@x -c user.name=t commit -q --allow-empty -m one
GREEN='echo "test result: ok. 5 passed; 0 failed; 0 ignored"'
CARGO_RE='^test result: '   # the result lines count only with --result-re

# G1: 3 green runs pass and print 3 result lines, each with its own log
LD="$T/g1"
out=$("$G" --runs 3 --cmd "$GREEN" --result-re "$CARGO_RE" --repo "$R" --log-dir "$LD" 2>/dev/null); rc=$?
n=$(printf '%s\n' "$out" | grep -c '^  test result: ok\. 5 passed')
ok=1; [ "$rc" -eq 0 ] && [ "$n" -eq 3 ] && [ -f "$LD/run-3.log" ] && printf '%s\n' "$out" | grep -q 'PASS: 3 of 3' && ok=0
case_result "G1 3 green runs: pass, 3 result lines printed" $ok "rc=$rc, lines=$n"

# G2: a red run stops the loop (run 2 of 4 prints a failure), no run 3
LD="$T/g2"; rm -f "$T/g2.count"
CMD='c=$(( $(cat '"$T/g2.count"' 2>/dev/null || echo 0) + 1 )); echo $c >'"$T/g2.count"'; if [ $c -eq 2 ]; then echo "test result: FAILED. 4 passed; 1 failed"; exit 101; fi; echo "test result: ok. 5 passed; 0 failed"'
out=$("$G" --runs 4 --cmd "$CMD" --repo "$R" --log-dir "$LD" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 1 ] && [ ! -e "$LD/run-3.log" ] && printf '%s\n' "$out" | grep -q 'RED at run 2 of 4' && ok=0
case_result "G2 a red run stops the loop and fails (exit 1, no run 3)" $ok "rc=$rc"

# G3: an OLD green line in the log dir never counts. (a) old green + new red -> red
LD="$T/g3"; mkdir -p "$LD"; echo "test result: ok. 99 passed; 0 failed" >"$LD/run-1.log"
out=$("$G" --runs 1 --cmd 'echo "test result: FAILED. 1 passed; 1 failed"' --result-re "$CARGO_RE" --repo "$R" --log-dir "$LD" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 1 ] && ! printf '%s\n' "$out" | grep -q '99 passed' && ok=0
case_result "G3 old green line + new red line in run-1.log is RED" $ok "rc=$rc"
# G3b: old green line, the new run prints no result line -> red (the old line was removed)
echo "test result: ok. 99 passed; 0 failed" >"$LD/run-1.log"
out=$("$G" --runs 1 --cmd 'echo building' --result-re "$CARGO_RE" --repo "$R" --log-dir "$LD" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 1 ] && printf '%s\n' "$out" | grep -q 'no result line' && ! printf '%s\n' "$out" | grep -q '99 passed' && ok=0
case_result "G3b old green line + no new result line is RED" $ok "rc=$rc"

# G4: a SHA change between runs fails (exit 2) and the changed run is not counted
LD="$T/g4"; rm -f "$T/g4.count"
CMD='c=$(( $(cat '"$T/g4.count"' 2>/dev/null || echo 0) + 1 )); echo $c >'"$T/g4.count"'; if [ $c -eq 2 ]; then git -c user.email=t@x -c user.name=t commit -q --allow-empty -m two; fi; echo "test result: ok. 5 passed; 0 failed"'
out=$("$G" --runs 3 --cmd "$CMD" --repo "$R" --log-dir "$LD" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 2 ] && printf '%s\n' "$out" | grep -q 'SHA CHANGED during run 2' && ! printf '%s\n' "$out" | grep -q 'PASS' && [ ! -e "$LD/run-3.log" ] && ok=0
case_result "G4 SHA change between runs fails (exit 2), not counted" $ok "rc=$rc"
# G4b: the same command with a stable SHA passes (other direction)
LD="$T/g4b"
out=$("$G" --runs 3 --cmd "$GREEN" --repo "$R" --log-dir "$LD" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 0 ] && ok=0
case_result "G4b stable SHA passes" $ok "rc=$rc"

# G5: a missing result line is red even if the exit code is 0
out=$("$G" --runs 2 --cmd 'echo hello' --result-re "$CARGO_RE" --repo "$R" --log-dir "$T/g5" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 1 ] && printf '%s\n' "$out" | grep -q 'RED at run 1 of 2' && ok=0
case_result "G5 exit 0 but no result line is RED, not green" $ok "rc=$rc"

# G6: exit 0 but a result line that reports a failure is red; a custom pattern works
out=$("$G" --runs 1 --cmd 'echo "test result: ok. 3 passed; 2 failed"' --result-re "$CARGO_RE" --repo "$R" --log-dir "$T/g6" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 1 ] && ok=0
case_result "G6 exit 0 with '2 failed' in the result line is RED" $ok "rc=$rc"
out=$("$G" --runs 2 --cmd 'echo "== 46 passed in 1.2s =="' --result-re '^== .*(passed|failed)' --repo "$R" --log-dir "$T/g6b" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 0 ] && ok=0
case_result "G6b --result-re for another runner passes on 2 green runs" $ok "rc=$rc"
out=$("$G" --runs 1 --cmd 'echo "== 1 failed, 46 passed in 1.2s =="' --result-re '^== .*(passed|failed)' --repo "$R" --log-dir "$T/g6c" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 1 ] && ok=0
case_result "G6c '1 failed, 46 passed' is RED" $ok "rc=$rc"

# G10 (B1): without --result-re a run is judged by the exit code alone, so any runner works.
# A Jest run (exit 0, "Tests: 5 passed") is green; the evidence says how it was judged.
JEST_OK='echo "Tests:       5 passed, 5 total"'
out=$("$G" --runs 2 --cmd "$JEST_OK" --repo "$R" --log-dir "$T/g10" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 0 ] && printf '%s\n' "$out" | grep -q 'PASS: 2 of 2' \
  && [ "$(printf '%s\n' "$out" | grep -c 'judged by the exit code')" -eq 2 ] && ok=0
case_result "G10 no --result-re: Jest exit 0 is green (exit code only)" $ok "rc=$rc"
# G10b: the other direction: without --result-re a non-zero exit is red at once
out=$("$G" --runs 3 --cmd 'echo "Tests: 1 failed, 4 passed, 5 total"; exit 1' --repo "$R" --log-dir "$T/g10b" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 1 ] && printf '%s\n' "$out" | grep -q 'RED at run 1 of 3: the command exited 1' && [ ! -e "$T/g10b/run-2.log" ] && ok=0
case_result "G10b no --result-re: exit 1 is RED, the loop stops" $ok "rc=$rc"
# G10c: with --result-re '^Tests: ' (Jest) a failed count is red even on exit 0, a clean line green
out=$("$G" --runs 1 --cmd 'echo "Tests:       1 failed, 4 passed, 5 total"' --result-re '^Tests: ' --repo "$R" --log-dir "$T/g10c" 2>/dev/null); rc1=$?
out=$("$G" --runs 1 --cmd "$JEST_OK" --result-re '^Tests: ' --repo "$R" --log-dir "$T/g10d" 2>/dev/null); rc2=$?
ok=1; [ "$rc1" -eq 1 ] && [ "$rc2" -eq 0 ] && printf '%s\n' "$out" | grep -q '^  Tests: ' && ok=0
case_result "G10c --result-re '^Tests: ': '1 failed' red, '5 passed' green" $ok "rc=$rc1/$rc2"
# G10d: pytest with --result-re: a summary line is green, a missing line is red
out=$("$G" --runs 1 --cmd 'echo "============ 5 passed in 0.12s ============"' --result-re '^=+ .*(passed|failed)' --repo "$R" --log-dir "$T/g10e" 2>/dev/null); rc1=$?
out=$("$G" --runs 1 --cmd 'echo collecting' --result-re '^=+ .*(passed|failed)' --repo "$R" --log-dir "$T/g10f" 2>/dev/null); rc2=$?
ok=1; [ "$rc1" -eq 0 ] && [ "$rc2" -eq 1 ] && ok=0
case_result "G10d pytest --result-re: summary green, no summary red" $ok "rc=$rc1/$rc2"
# G10e: --help shows the Jest and the pytest example
h=$("$G" --help)
ok=1; printf '%s' "$h" | grep -q "Jest: *--result-re '\^Tests: '" && printf '%s' "$h" | grep -q 'pytest: *--result-re' && ok=0
case_result "G10e --help shows a Jest and a pytest example" $ok "jest=$(printf '%s' "$h" | grep -c Jest) pytest=$(printf '%s' "$h" | grep -c pytest)"

# G7: with --lock: each run holds the host lock; the lock is free after; a live foreign lock stops the run
LK="$T/g7.lock.d"
CMD='bash '"$L"' status '"$LK"' --no-board >>'"$T/g7.during"'; echo "test result: ok. 1 passed; 0 failed"'
rm -f "$T/g7.during"
out=$("$G" --runs 2 --cmd "$CMD" --lock "$LK" --holder gl@x --repo "$R" --log-dir "$T/g7" 2>/dev/null); rc=$?
held=$(grep -c '^held: gl@x@hostA' "$T/g7.during")
ok=1; [ "$rc" -eq 0 ] && [ "$held" -eq 2 ] && [ ! -e "$LK" ] && ok=0
case_result "G7 --lock: the lock is held during each run, free after" $ok "rc=$rc, held in $held of 2 runs"
LK="$T/g7b.lock.d"; plant "$LK" other@x hostA $LIVE "$NOW"
out=$("$G" --runs 1 --cmd 'touch '"$T/g7b.ran"'; '"$GREEN" --lock "$LK" --holder gl@x --lock-wait 0 --repo "$R" --log-dir "$T/g7b" 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 1 ] && [ ! -e "$T/g7b.ran" ] && grep -q '^agent=other@x$' "$LK/owner" && ok=0
case_result "G7b --lock held by a live owner: the run does not start" $ok "rc=$rc"

# G9: CI first. gate-loop pauses while Runner.Worker runs and goes on when it is gone
echo "$RUNNER_LINE" >"$T/ps.txt"
( sleep 2; : >"$T/ps.txt" ) &
out=$("$G" --runs 2 --cmd "$GREEN" --repo "$R" --log-dir "$T/g9" --ci-poll 1 2>"$T/g9.err"); rc=$?
wait
ok=1; [ "$rc" -eq 0 ] && grep -q 'run 1 paused' "$T/g9.err" && printf '%s\n' "$out" | grep -q 'run 1: paused .* for CI, then went on' \
  && ! printf '%s\n' "$out" | grep -q 'run 2: paused' && ok=0
case_result "G9 pauses while CI runs, resumes after, evidence notes it" $ok "rc=$rc"
# G9b: CI never goes: exit 3 after --ci-max-wait and the command never ran
echo "$RUNNER_LINE" >"$T/ps.txt"
out=$("$G" --runs 1 --cmd 'touch '"$T/g9b.ran"'; '"$GREEN" --repo "$R" --log-dir "$T/g9b" --ci-poll 1 --ci-max-wait 2 2>/dev/null); rc=$?
ok=1; [ "$rc" -eq 3 ] && [ ! -e "$T/g9b.ran" ] && ok=0
case_result "G9b CI that never ends: exit 3, command not run" $ok "rc=$rc"
# G9c: a waiting-list entry next to --lock also pauses; no CI means no pause
: >"$T/ps.txt"; LK="$T/g9c.lock.d"; "$L" ci-wait "$LK" --holder ci-x 2>/dev/null
( sleep 2; "$L" ci-done "$LK" --holder ci-x ) &
out=$("$G" --runs 1 --cmd "$GREEN" --lock "$LK" --holder gl@x --repo "$R" --log-dir "$T/g9c" --ci-poll 1 2>"$T/g9c.err"); rc=$?
wait
ok=1; [ "$rc" -eq 0 ] && grep -q 'waiting list entry ci-x' "$T/g9c.err" && ok=0
case_result "G9c waiting-list entry pauses the loop; ci-done resumes it" $ok "rc=$rc"
out=$("$G" --runs 1 --cmd "$GREEN" --repo "$R" --log-dir "$T/g9d" --ci-poll 1 2>"$T/g9d.err"); rc=$?
ok=1; [ "$rc" -eq 0 ] && ! grep -q paused "$T/g9d.err" && ok=0
case_result "G9d no CI: no pause" $ok "rc=$rc"

# G8: usage errors
"$G" --runs 0 --cmd x >/dev/null 2>&1; r1=$?
"$G" --runs 2 >/dev/null 2>&1; r2=$?
ok=1; [ "$r1" -eq 64 ] && [ "$r2" -eq 64 ] && ok=0
case_result "G8 bad --runs / missing --cmd -> 64" $ok "rc=$r1/$r2"

# E: --expect, overdue, pid=0 notes, durations, board check with a config dir
backdate() { # dir seconds: rewrite started_epoch to "that many seconds ago"
  local e=$(( $(date +%s) - $2 ))
  sed "s/^started_epoch=.*/started_epoch=$e/" "$1/owner" >"$1/owner.new" && mv "$1/owner.new" "$1/owner"
}
D="$T/e1"
"$L" take "$D" --holder a@x --no-board --pid 0 2>"$T/e1.err"; rc=$?
ok=1; [ "$rc" -eq 64 ] && [ ! -e "$D" ] && grep -q -- '--expect' "$T/e1.err" && grep -q "'run'" "$T/e1.err" && ok=0
case_result "E1 take --pid 0 without --expect: 64, names run, no lock" $ok "rc=$rc"
"$L" take "$D" --holder a@x --no-board --pid 0 --expect 1 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q '^pid=0$' "$D/owner" && grep -q '^expect=1$' "$D/owner" && ok=0
case_result "E1b take --pid 0 --expect 1: taken, expect=1 in the owner file" $ok "rc=$rc"
"$L" release "$D" --holder a@x 2>/dev/null
D="$T/e1c"; "$L" take "$D" --holder a@x --no-board --pid "$LIVE" 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 0 ] && ! grep -q '^expect=' "$D/owner" && ok=0
case_result "E1c take with a pid needs no --expect (no expect= line)" $ok "rc=$rc"

# E2: the deadline is expect x 2 + 5 min = 302 s for --expect 1
D="$T/e2"; "$L" take "$D" --holder a@x --no-board --pid 0 --expect 1 2>/dev/null
backdate "$D" 290
"$L" status "$D" >"$T/e2.out" 2>&1; rc1=$?
"$L" take "$D" --holder b@x --no-board 2>/dev/null; rc2=$?
ok=1; [ "$rc1" -eq 0 ] && [ "$rc2" -eq 75 ] && grep -q '^agent=a@x$' "$D/owner" && ok=0
case_result "E2 expect 1, 290 s old: still held (status 0, take 75)" $ok "status=$rc1 take=$rc2"
backdate "$D" 310
"$L" status "$D" >"$T/e2b.out" 2>&1; rc1=$?
"$L" take "$D" --holder b@x --no-board 2>"$T/e2b.err"; rc2=$?
ok=1; [ "$rc1" -eq 2 ] && grep -q '^stale: overdue' "$T/e2b.out" && [ "$rc2" -eq 0 ] && grep -q 'reason=overdue' "$T/e2b.err" && grep -q '^agent=b@x$' "$D/owner" && ok=0
case_result "E2b expect 1, 310 s old: stale (overdue), taken over" $ok "status=$rc1 take=$rc2"

# E3: an old owner file (no expect=) keeps the 4 h rule; a judged pid is never overdue
D="$T/e3"; plant "$D" a@x hostA 0 $((NOW - 2000))
"$L" take "$D" --holder b@x --no-board 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 75 ] && ok=0
case_result "E3 pid=0 without expect=, 2000 s old: held (old rule)" $ok "rc=$rc"
D="$T/e3b"; plant "$D" a@x hostA 0 $((NOW - 20000))
"$L" take "$D" --holder b@x --no-board 2>"$T/e3b.err"; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q 'reason=too-old' "$T/e3b.err" && ok=0
case_result "E3b pid=0 without expect=, 20000 s old: too-old after 4 h" $ok "rc=$rc"
D="$T/e3c"; plant "$D" a@x hostA "$LIVE" $((NOW - 5000)); echo 'expect=1' >>"$D/owner"
"$L" take "$D" --holder b@x --no-board 2>/dev/null; rc=$?
ok=1; [ "$rc" -eq 75 ] && ok=0
case_result "E3c live pid with expect=1, 5000 s old: not overdue (pid judged)" $ok "rc=$rc"
D="$T/e3d"; plant "$D" a@x hostB "$LIVE" $((NOW - 5000)); echo 'expect=1' >>"$D/owner"
"$L" take "$D" --holder b@x --no-board 2>"$T/e3d.err"; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q 'reason=overdue' "$T/e3d.err" && ok=0
case_result "E3d other host with expect=1, 5000 s old: overdue" $ok "rc=$rc"

# E4: the warnings in status and in the wait message
D="$T/e4"; plant "$D" a@x hostA 0 $((NOW - 600)); echo 'expect=300' >>"$D/owner"
backdate "$D" 560   # 260 s past expect=300: 5 min rounded up, even if the runner is 40 s slow
"$L" status "$D" --no-board >"$T/e4.out" 2>&1; rc=$?
ok=1; [ "$rc" -eq 0 ] && grep -q 'pid=0: a dead holder cannot be detected' "$T/e4.out" && grep -q 'overdue by 5 min' "$T/e4.out" && ok=0
case_result "E4 status: pid=0 note and overdue by 5 min" $ok "rc=$rc: $(head -n 1 "$T/e4.out" | cut -c1-110)"
"$L" take "$D" --holder b@x --no-board --wait 2 --poll 0.3 2>"$T/e4b.err"; rc=$?
ok=1; [ "$rc" -eq 75 ] && grep -q 'waiting for lock.*pid=0: a dead holder cannot be detected' "$T/e4b.err" && grep -q 'overdue by 5 min' "$T/e4b.err" && ok=0
case_result "E4b wait message: both warnings" $ok "rc=$rc $(tr '\n' '~' <"$T/e4b.err" | cut -c1-300)"
D="$T/e4c"; plant "$D" a@x hostA 0 $((NOW - 10)); echo 'expect=3600' >>"$D/owner"
"$L" status "$D" --no-board >"$T/e4c.out" 2>&1
ok=1; grep -q 'pid=0: a dead holder cannot be detected' "$T/e4c.out" && ! grep -q 'overdue' "$T/e4c.out" && ok=0
case_result "E4c before expect: the pid=0 note, no overdue" $ok ""
D="$T/e4d"; plant "$D" a@x hostA "$LIVE" $((NOW - 10))
"$L" status "$D" --no-board >"$T/e4d.out" 2>&1
ok=1; ! grep -q 'cannot be detected' "$T/e4d.out" && ok=0
case_result "E4d a real pid: no pid=0 note" $ok ""

# E5: durations: 90, 90s, 30m, 2h, everywhere
for pair in "90:90" "90s:90" "30m:1800" "2h:7200"; do
  v=${pair%%:*}; want=${pair##*:}; D="$T/e5-$v"
  "$L" take "$D" --holder a@x --no-board --pid 0 --expect "$v" --wait "$v" --max-age "$v" 2>/dev/null; rc=$?
  ok=1; [ "$rc" -eq 0 ] && grep -q "^expect=$want\$" "$D/owner" && ok=0
  case_result "E5 --expect $v records expect=$want" $ok "rc=$rc"
done
"$L" take "$T/e5x" --holder a@x --no-board --pid 0 --expect 2d >/dev/null 2>&1; r1=$?
"$L" take "$T/e5y" --holder a@x --no-board --wait soon >/dev/null 2>&1; r2=$?
"$L" take "$T/e5z" --holder a@x --no-board --pid 0 --expect m >/dev/null 2>&1; r3=$?
ok=1; [ "$r1" -eq 64 ] && [ "$r2" -eq 64 ] && [ "$r3" -eq 64 ] && [ ! -e "$T/e5x" ] && ok=0
case_result "E5b bad durations (2d, soon, m): 64" $ok "rc=$r1/$r2/$r3"

# E6: the board check and the agent config dir
if :; then   # the board check runs in the binary: no jq
  export FAKE_STATUS="$T/status6.json" FAKE_ARGV="$T/e6.argv"
  status_json "$FAKE_STATUS" "own@x:dead:active" "alive@x:alive:active"
  D="$T/e6"; plant "$D" alive@x hostA "$LIVE" "$NOW"; : >"$FAKE_ARGV"
  CREWBOARD_CONFIG_DIR="$T/agentdir" "$L" take "$D" --holder b@x 2>/dev/null; rc=$?
  ok=1; [ "$rc" -eq 75 ] && [ "$(head -n 1 "$FAKE_ARGV")" = "--config-dir $T/agentdir status --json" ] && ok=0
  case_result "E6 CREWBOARD_CONFIG_DIR: crewboard --config-dir <dir> status" $ok "argv=$(head -n 1 "$FAKE_ARGV")"
  : >"$FAKE_ARGV"
  CREWBOARD_CONFIG_DIR="$T/envdir" "$L" take "$D" --holder b@x --config-dir "$T/optdir" --initiative i1 2>/dev/null
  ok=1; [ "$(head -n 1 "$FAKE_ARGV")" = "--config-dir $T/optdir status --json --initiative i1" ] && ok=0
  case_result "E6b --config-dir beats the variable; --initiative is passed" $ok "argv=$(head -n 1 "$FAKE_ARGV")"
  : >"$FAKE_ARGV"; "$L" take "$D" --holder b@x 2>/dev/null
  ok=1; [ "$(head -n 1 "$FAKE_ARGV")" = "status --json" ] && ok=0
  case_result "E6c neither set: no --config-dir argument" $ok "argv=$(head -n 1 "$FAKE_ARGV")"
  # a crewboard that finds no board config without --config-dir (the real failure of the issue)
  export FAKE_NOCONFIG=1
  D="$T/e6d"; plant "$D" own@x hostA "$LIVE" "$NOW"
  "$L" status "$D" >"$T/e6d.out" 2>&1; rc=$?
  n=$(grep -c 'board check unavailable: no agent config dir' "$T/e6d.out")
  ok=1; [ "$rc" -eq 0 ] && [ "$n" -eq 1 ] && ! grep -q 'board check skipped' "$T/e6d.out" && ok=0
  case_result "E6d no config: status says 'unavailable' once, no failure note" $ok "rc=$rc, lines=$n"
  "$L" take "$D" --holder b@x --wait 1 --poll 0.3 2>"$T/e6e.err"; rc=$?
  ok=1; [ "$rc" -eq 75 ] && ! grep -q 'board check' "$T/e6e.err" && ok=0
  case_result "E6e no config: take and wait stay silent about it" $ok "rc=$rc"
  CREWBOARD_CONFIG_DIR="$T/agentdir" "$L" take "$D" --holder b@x 2>"$T/e6f.err"; rc=$?
  ok=1; [ "$rc" -eq 0 ] && grep -q 'reason=board-dead' "$T/e6f.err" && ok=0
  case_result "E6f with the config dir the board check works: board-dead takeover" $ok "rc=$rc"
  unset FAKE_NOCONFIG FAKE_STATUS FAKE_ARGV
fi

# (mtime_of is gone: poll-and-wake.sh is a shim for `crewboard wake`, whose stop file check uses
# std::fs::metadata; host-lock.sh is a shim too, proved by the Rust tests lock::fsmeta::m1_the_epoch_is_exact
# and m3_a_missing_path_is_none)

# ---------------- detached jobs, atomic release, gates ----------------
echo "== host-lock.sh --detach, release, --gate"
: >"$T/ps.txt"
wait_file() { # file, seconds: 0 when the file exists
  local i=0; while [ ! -e "$1" ] && [ "$i" -lt $(($2 * 10)) ]; do sleep 0.1; i=$((i + 1)); done; [ -e "$1" ]
}
DD="$T/detach"; mkdir -p "$DD"
# DT1: --detach returns at once; the job runs on and holds the lock; the owner file names the job pid
t0=$(date +%s)
out=$(bash "$L" run "$DD/l1" --holder a@hostA --reason job --no-board --detach --result "$DD/r1" -- bash -c 'sleep 4; exit 5' 2>/dev/null); rc=$?
dur=$(( $(date +%s) - t0 ))
jp=$(printf '%s' "$out" | sed -n 's/^detached: pid=\([0-9]*\) .*/\1/p')
ok=1; [ "$rc" -eq 0 ] && [ "$dur" -lt 3 ] && [ -n "$jp" ] && kill -0 "$jp" 2>/dev/null && [ ! -e "$DD/r1" ] \
  && grep -q "^pid=$jp\$" "$DD/l1/owner" && grep -q '^detached=1$' "$DD/l1/owner" \
  && printf '%s' "$out" | grep -q "log=.*r1.log result=.*r1\$" && ok=0
case_result "DT1 detach: returns at once, job holds the lock, owner has its pid" $ok "rc=$rc, ${dur}s, out=$out"
bash "$L" run "$DD/l1" --holder b@hostA --no-board -- true >/dev/null 2>&1; r2=$?
st=$(bash "$L" status "$DD/l1" --no-board 2>/dev/null); rs=$?
ok=1; [ "$r2" -eq 75 ] && [ "$rs" -eq 0 ] && printf '%s' "$st" | grep -q 'detached job' && ok=0
case_result "DT1b detached lock: another taker gets 75, status says detached" $ok "take rc=$r2, status rc=$rs"
# DT2: the result file appears with rc, end and log when the job ends; then the lock is free
wait_file "$DD/r1" 12; i=0; while [ -e "$DD/l1" ] && [ "$i" -lt 30 ]; do sleep 0.1; i=$((i + 1)); done   # the lock goes right after the result
got=$(cat "$DD/r1" 2>/dev/null)
ok=1; printf '%s' "$got" | grep -Eq "^rc=5 end=[0-9TZ:-]+ log=.*r1\.log\$" && [ ! -e "$DD/l1" ] && ok=0
case_result "DT2 detach: result file rc=5 end= log=, lock released" $ok "result=$got, lock left=$([ -e "$DD/l1" ] && echo yes || echo no)"
# DT2b: --ticket and --sha add the lines ticket= and sha=; the first line stays as it was
out=$(bash "$L" run "$DD/l2b" --holder a@hostA --no-board --detach --result "$DD/r2b" --ticket 42 --sha abc123 -- bash -c 'exit 0' 2>/dev/null); rc=$?
wait_file "$DD/r2b" 12; i=0; while [ -e "$DD/l2b" ] && [ "$i" -lt 30 ]; do sleep 0.1; i=$((i + 1)); done
got=$(cat "$DD/r2b" 2>/dev/null)
ok=1; [ "$rc" -eq 0 ] && printf '%s\n' "$got" | sed -n 1p | grep -Eq "^rc=0 end=[0-9TZ:-]+ log=.*r2b\.log\$" \
  && printf '%s\n' "$got" | grep -qx 'ticket=42' && printf '%s\n' "$got" | grep -qx 'sha=abc123' && ok=0
case_result "DT2b detach --ticket --sha: result file has ticket= and sha=" $ok "rc=$rc, result=$(printf '%s' "$got" | tr '\n' '|')"
bash "$L" run "$DD/l2c" --holder a@hostA --no-board --detach --result "$DD/r2c" --ticket x1 -- true >/dev/null 2>&1; rcb=$?
bash "$L" run "$DD/l2c" --holder a@hostA --no-board --ticket 5 -- true >/dev/null 2>&1; rcc=$?
ok=1; [ "$rcb" -ne 0 ] && [ "$rcc" -ne 0 ] && [ ! -e "$DD/r2c" ] && ok=0
case_result "DT2c --ticket needs digits and --detach" $ok "rc=$rcb/$rcc"
# DT3: a killed wake (the budget stop: TERM, then KILL, to the process group) leaves the detached job
# running with its lock; the job ends later by itself
rm -f "$DD/r3"
( set -m
  sh -c "bash '$L' run '$DD/l3' --holder a@hostA --no-board --detach --result '$DD/r3' -- bash -c 'sleep 4; echo done3' >'$DD/w3.out' 2>&1; sleep 60" &
  WP=$!; set +m
  wait_file "$DD/w3.out" 5; sleep 1
  kill -TERM -- "-$WP" 2>/dev/null; sleep 0.5; kill -KILL -- "-$WP" 2>/dev/null; wait "$WP" 2>/dev/null ) 2>/dev/null
jp=$(sed -n 's/^pid=//p' "$DD/l3/owner" 2>/dev/null)
ok=1; [ -n "$jp" ] && kill -0 "$jp" 2>/dev/null && grep -q '^detached=1$' "$DD/l3/owner" && [ ! -e "$DD/r3" ] && ok=0
case_result "DT3 killed wake: detached job and its lock stay" $ok "job pid=$jp alive=$(kill -0 "$jp" 2>/dev/null && echo yes || echo no)"
wait_file "$DD/r3" 12; i=0; while [ -e "$DD/l3" ] && [ "$i" -lt 30 ]; do sleep 0.1; i=$((i + 1)); done; ok=1
grep -q '^rc=0 ' "$DD/r3" 2>/dev/null && grep -q done3 "$DD/r3.log" && [ ! -e "$DD/l3" ] && ok=0
case_result "DT3b the job finishes after the wake: result rc=0, lock released" $ok "result=$(cat "$DD/r3" 2>/dev/null)"
# DT4: usage: --detach needs --result and run; --result needs --detach
bash "$L" run "$DD/l4" --holder a@hostA --detach -- true >/dev/null 2>&1; u1=$?
bash "$L" run "$DD/l4" --holder a@hostA --result "$DD/r4" -- true >/dev/null 2>&1; u2=$?
bash "$L" take "$DD/l4" --holder a@hostA --detach --result "$DD/r4" >/dev/null 2>&1; u3=$?
ok=1; [ "$u1" -eq 64 ] && [ "$u2" -eq 64 ] && [ "$u3" -eq 64 ] && [ ! -e "$DD/l4" ] && ok=0
case_result "DT4 --detach without --result / --result without --detach -> 64" $ok "rc=$u1/$u2/$u3"
# DT5 (release is atomic: no moment with an empty lock dir) moved to Rust:
# lock::run::dt5_a_release_renames_before_it_removes_so_no_empty_dir_is_ever_seen. The old proof put
# stubs for `rm` and `rmdir` on the PATH; the Rust code calls neither.
# DT6: the other direction: a release by another owner leaves the lock complete (exit 3)
plant "$DD/l6" a@hostA hostA "$LIVE" "$NOW"
bash "$L" release "$DD/l6" --holder z@hostA >/dev/null 2>&1; rc=$?
ok=1; [ "$rc" -eq 3 ] && [ -s "$DD/l6/owner" ] && ok=0
case_result "DT6 release by a stranger: exit 3, lock complete" $ok "rc=$rc"
# DT7 (the job is in its own session) moved to Rust: lock_detach::dt7_child_is_session_leader.
# The shell had three ways to a new session (setsid, perl, set -m); the binary calls setsid itself.
# GT1: --gate does not wait for a CI job on the host; plain run does (76). A held lock still blocks --gate (75).
echo "$RUNNER_LINE" >"$T/ps.txt"
rm -f "$DD/ran"
bash "$L" run "$DD/l7" --holder a@hostA --no-board -- touch "$DD/ran.plain" >/dev/null 2>&1; rp=$?
bash "$L" run "$DD/l7" --holder a@hostA --no-board --gate -- touch "$DD/ran.gate" >/dev/null 2>&1; rg=$?
ok=1; [ "$rp" -eq 76 ] && [ ! -e "$DD/ran.plain" ] && [ "$rg" -eq 0 ] && [ -e "$DD/ran.gate" ] && [ ! -e "$DD/l7" ] && ok=0
case_result "GT1 CI job runs: plain run -> 76, run --gate runs the job" $ok "plain rc=$rp, gate rc=$rg"
plant "$DD/l8" b hostA "$LIVE" "$NOW"
bash "$L" run "$DD/l8" --holder a@hostA --no-board --gate -- true >/dev/null 2>&1; rg=$?
ok=1; [ "$rg" -eq 75 ] && ok=0
case_result "GT2 --gate still waits for a held lock (75)" $ok "rc=$rg"
# GT3: --gate with --detach also skips the CI wait
out=$(bash "$L" run "$DD/l9" --holder a@hostA --no-board --gate --detach --result "$DD/r9" -- bash -c 'exit 0' 2>/dev/null); rg=$?
wait_file "$DD/r9" 8
ok=1; [ "$rg" -eq 0 ] && grep -q '^rc=0 ' "$DD/r9" 2>/dev/null && ok=0
case_result "GT3 --gate --detach skips the CI wait" $ok "rc=$rg, result=$(cat "$DD/r9" 2>/dev/null)"
out=$(bash "$L" run "$DD/l10" --holder a@hostA --no-board --detach --result "$DD/r10" -- true 2>/dev/null); rg=$?
ok=1; [ "$rg" -eq 76 ] && [ ! -e "$DD/l10" ] && [ ! -e "$DD/r10" ] && ok=0
case_result "GT4 --detach without --gate waits for CI first (76)" $ok "rc=$rg"
: >"$T/ps.txt"
# OP1-OP3: an option that does not apply to the subcommand is refused with exit 64 (it was ignored)
plant "$DD/l13" b hostA "$LIVE" "$NOW"
bash "$L" release "$DD/l13" --holder b@hostA --wait 60 >/dev/null 2>"$T/op1.err"; rc=$?
ok=1; [ "$rc" -eq 64 ] && [ -d "$DD/l13" ] && grep -q -- '--wait does not apply to release' "$T/op1.err" && ok=0
case_result "OP1 release --wait: exit 64, the lock stays" $ok "rc=$rc"
bash "$L" status "$DD/l13" --no-board --pid 1 >/dev/null 2>"$T/op2.err"; rc=$?
ok=1; [ "$rc" -eq 64 ] && grep -q -- '--pid does not apply to status' "$T/op2.err" && ok=0
case_result "OP2 status --pid: exit 64" $ok "rc=$rc"
bash "$L" break "$DD/l13" --if-dead --reason x >/dev/null 2>"$T/op3.err"; rc=$?
ok=1; [ "$rc" -eq 64 ] && [ -d "$DD/l13" ] && grep -q -- '--reason does not apply to break' "$T/op3.err" && ok=0
case_result "OP3 break --reason: exit 64, the lock stays" $ok "rc=$rc"

echo
[ "${CASES:-0}" -gt 0 ] || { echo "SUMMARY: FAILED (no case ran)"; exit 1; }
echo "cases run: $CASES"
if [ "$FAIL" -eq 0 ]; then echo "SUMMARY: all cases passed"; exit 0; else echo "SUMMARY: FAILED"; exit 1; fi
