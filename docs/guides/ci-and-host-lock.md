# The host lock

A host that several agents share can run only one heavy job at a time. `crewboard lock` is the
lock for it. The owner edits the CI workflow side of it; for the rest of the CI setup, read the
[CI for crew pull requests](ci-for-crew-prs.md).

**Prerequisites:** the `crewboard` binary and `ps` on the host (Linux and macOS). No `bash` and no
`jq` are needed: the lock is the command `crewboard lock`, and its board check runs inside it. The CI
runner and the agents must run as one user with one `HOME`, so that they share the lock directory.
If they do not, put the lock directory in a path that both users can read and write (for example
a group-writable directory) and give that path to every `crewboard lock` call. (A lock of another
user is seen as alive, so a runner of another user does not make its lock stale.)

## CI steps and the host lock

A CI job on a shared host goes before the agents ([CI first](../design/spec/host-lock.md#ci-first-empty-lock-grace-named-reasons)). The job registers in the waiting
list before it waits for the lock, takes the lock as CI (a waiting agent never blocks it), and
leaves the list at the end. Put this in the workflow of the job (the owner edits workflows):

```yaml
- run: crewboard lock ci-wait ~/.cache/<initiative>/heavy.lock.d --holder "ci-${{ github.run_id }}" --pid runner
- run: crewboard lock take ~/.cache/<initiative>/heavy.lock.d --ci --holder "ci-${{ github.run_id }}" --wait 1200 --pid runner
# ... the heavy steps ...
- if: always()
  run: |
    crewboard lock release ~/.cache/<initiative>/heavy.lock.d --holder "ci-${{ github.run_id }}"
    crewboard lock ci-done ~/.cache/<initiative>/heavy.lock.d --holder "ci-${{ github.run_id }}"
```

The first step is the one that matters: from then on no agent takes the lock. `take --ci`
also registers while it waits, so the first step is the safe extra. The runner process
(`Runner.Worker`) is seen without any step.

## Agents: `run`, or `--pid 0 --expect`

Best form: `crewboard lock run <dir> --holder <you> -- <cmd>`. The lock lives exactly as long as the
command. If you must take and release in two calls (the shell of a tool call ends with the call),
use `take <dir> --holder <you> --pid 0 --expect 10m` and `release` later. With `pid=0` nobody can
prove a dead holder dead, so the lock is stale only after `expect x 2 + 5 min` (reason `overdue`).
`--expect 10m` frees a forgotten lock after 25 minutes, not after 4 hours. Durations are `90`,
`90s`, `30m` or `2h`. `status` shows `overdue by <n> min` once you are past your estimate.

For the board check (dead owner agent), an agent whose config is not in the default dir sets
`CREWBOARD_CONFIG_DIR=<config dir of the agent>` or passes the global `--config-dir`
(`crewboard --config-dir <dir> lock ...`). Without it `status`
prints `board check unavailable: no agent config dir`, and the pid and age checks go on.

Never delete a lock dir or its `owner` file by hand. A held lock shows `alive=yes|no|unknown` and `age=<s>`.
`break <dir> --if-dead` removes the lock only if `take` would judge it stale. A live lock: exit 3, nothing changes.

## A tool that takes the lock itself

A project tool (a build script, a test runner) can take the same lock dir as the agent does.
If the agent wraps such a tool in `crewboard lock run <dir> -- <tool>`, the tool waits for the
lock that its own parent holds ([re-entrancy](../design/spec/host-lock.md#re-entrancy-a-tool-under-the-lock)).
Follow one of these rules.

1. **Do not wrap it.** Call the tool without `run`. The tool takes and releases the lock.
2. **Let the tool call `crewboard lock`.** This is the best form. `run` tells its child which
   lock it holds, so a nested `take` or `run` for the same dir passes at once (exit 0, one
   log line, no second owner file), and the nested `release` leaves the outer lock alone:

   ```bash
   # inside the tool
   LOCK=~/.cache/<initiative>/heavy.lock.d
   crewboard lock run "$LOCK" --holder "build-tool" --wait 3600 -- make heavy
   ```

3. **Honor the variable.** A tool with its own `mkdir` lock reads `CREWBOARD_HOST_LOCK` (the
   lock dir, resolved) and `CREWBOARD_HOST_LOCK_PID` (the holder). If the variable names the
   tool's lock dir, the caller holds it: skip the lock and do not remove it at the end.

   ```bash
   LOCK=~/.cache/<initiative>/heavy.lock.d
   if [ -n "${CREWBOARD_HOST_LOCK:-}" ] && [ "$(cd "$LOCK" 2>/dev/null && pwd -P)" = "$CREWBOARD_HOST_LOCK" ]; then
     echo "lock $LOCK is held by my caller (pid ${CREWBOARD_HOST_LOCK_PID:-?})"
     exec "$@"            # no lock of our own, nothing to release
   fi
   # ... own mkdir lock as before ...
   ```

The variables exist only in the command of `run`. If a wrapper cleans the environment, the
nested call finds that its ancestor holds the lock but has no grant. It exits 77 at once with
"held by your own parent process <pid>" and does not wait. Exit 77 is a setup error: fix the
wrapper, or use rule 1.

## A tool that makes the lock dir itself: the foreign holder

A tool or CI step that does its own `mkdir` on the lock path and puts only an info file in it is
a foreign holder ([spec](../design/spec/host-lock.md#foreign-holder-a-lock-without-an-owner-file)).
`crewboard lock` does not take it over after 30 s. It waits until the pid recorded in a `pid=<n>` line
is dead, or until `--max-age` (4 h by default) passes. A foreign holder without a `pid=` line
therefore costs the other jobs up to `--max-age`.

Better: write an `owner` file, or call `crewboard lock`. The `owner` file is `key=value` lines:
`agent`, `host`, `pid`, `started`, `started_epoch`, `reason`. A non-empty `agent=` and a numeric
`started_epoch=` are required. Without them the file is
[not valid](../design/spec/host-lock.md#invalid-owner-file-no-agent-or-start-time): `crewboard lock` judges it as a
foreign holder, and `release` does not remove it. Three lines of shell:

```bash
mkdir "$LOCK" || exit 75
printf 'agent=%s\nhost=%s\npid=%s\nstarted=%s\nstarted_epoch=%s\nreason=%s\n' "my-tool" "$(hostname -s)" "$$" "$(date -u +%FT%TZ)" "$(date +%s)" "e2e run" >"$LOCK/owner"
trap 'rm -f "$LOCK/owner"; rmdir "$LOCK"' EXIT
```

If you cannot write `owner`, add `pid=$$` (and `host=$(hostname -s)`) to your info file.
Then a dead tool frees the lock at once.

### Why `--pid runner`

The take and the release are separate steps, and the default pid is the shell of the step, which
ends with the step. `--pid runner` walks up the parent processes of `crewboard lock` to the
nearest `Runner.Worker` and records its pid. `Runner.Worker` lives exactly as long as the job. When
the job dies, the pid-dead rule frees the lock at the next agent `take` (same host only). An entry
of `ci-wait` with a dead pid is ignored and removed, so it no longer blocks the agents ("CI first",
exit 76).

If no `Runner.Worker` is a parent (the step runs outside GitHub Actions), `--pid runner` exits
with 64 and writes nothing. Use `--pid <number>` or `--pid 0 --expect <duration>` there. `--pid 0`
needs `--expect <duration>` (exit 64 without it).
