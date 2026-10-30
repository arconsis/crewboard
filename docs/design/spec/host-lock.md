# Host lock and repeated Gate runs

Part of the [crewboard specification](README.md).

## Host lock helper


A dead agent that holds a host lock (`heavy.lock.d`) must not block every other agent on the host
until a human looks. `crewboard lock` takes the lock and frees a stale one. It is a command of
the CLI. It needs neither `jq` nor `bash`.
The command needs a Unix host (macOS or Linux); on another platform it refuses with the rule
`unix-only` (exit 2).

1. **Subcommands.** `take`, `release`, `status`, `break --if-dead`, `ci-wait`, `ci-done`,
   `ci-status`, and `run <dir> --holder A -- CMD...` (holds the lock exactly as long as CMD runs
   and releases it when CMD ends or when `run` gets TERM, INT or HUP). `run` does not forward a
   signal to CMD: CMD is in the same process group, so a TERM or INT of the group (the budget stop
   of a wake, Ctrl-C) reaches it as before, and `run` then waits for CMD, releases the lock and
   exits 143. The lock is a directory;
   its `owner` file has agent, host, pid, started (UTC), started_epoch and reason (and `expect`,
   `detached=1` when they apply). `crewboard lock --help` prints the full text.
2. **Stale lock.** Any one of: the pid is gone (only on the same host; on another host a pid
   is never judged; a pid of another user counts as alive, because the signal check gives EPERM
   for it); the lock is older than `--max-age` (default 14400 s, `HOST_LOCK_MAX_AGE`,
   `0` = no limit); the board says the owner agent is `dead` or its State is `left`. The board
   check runs in the command, with the same answer as `crewboard status --json`. It is skipped
   with one note if the board or the config is not reachable, or the agent is not listed
   ([config dir](#expect-and-overdue-a-lock-without-a-provable-holder)); the lock directory is EMPTY (no owner file, no other file) after a short grace time.
   A directory with another file but no owner file is a foreign holder
   ([Foreign holder](#foreign-holder-a-lock-without-an-owner-file)), not an empty one. An owner
   file without a non-empty `agent=` or a numeric `started_epoch=` is not valid and is judged as a
   foreign holder too ([Invalid owner file](#invalid-owner-file-no-agent-or-start-time)).
3. **Takeover.** One taker at a time passes the guard `<dir>.takeover` (a `mkdir`), checks again
   that the lock is stale, renames it to a unique name, and removes the renamed copy.
   Then it makes its own lock with `mkdir`. The takeover prints one log line with the old owner
   and the reason. No `rm` then `mkdir` on the live name.
4. **Release** removes the lock only if the owner file is valid and names the same agent and
   host; otherwise exit 3 and the lock stays.
5. **Exit codes.** The codes of `lock` (also 75 held by a live owner, 76 CI first, 77 held by
   your own ancestor process) are in the [exit codes reference](../../reference/exit-codes.md#codes-of-the-host-tools).
6. **pid.** `take` records `--pid` (default: its parent process). A tool call of an agent ends
   its shell, so use `run`, or use `--pid 0 --expect <d>` for take and release in two calls
   ([Expect and overdue](#expect-and-overdue-a-lock-without-a-provable-holder)).
7. **Wake and headless scope.** `run` starts a command, and the wake allowlist is `Bash(crewboard *)`
   for every agent. So under a scope (a headless config dir, the system scope file, or a wake: the
   variable `CREWBOARD_WAKE` that `crewboard wake` sets) `lock run <dir>` is allowed only when `<dir>`
   is in `CREWBOARD_LOCK_RUN_DIRS`, a `:` list that the wake sets from its `--host-lock` dirs (symlink
   spellings count as one dir). Any other `run` is refused with the rule `lock-run-not-granted`
   (exit 2). A `crewboard ...` command line of the model cannot set the variables, because the
   allowlist matches only commands that start with `crewboard`. The other subcommands (`take`,
   `release`, `status`, `break`, `ci-*`) stay allowed.
8. Proof: the Rust tests in `src/lock/` (the stale rules on fake time and processes) and
   `tests/component/lock_*.rs` (the binary on a temp dir; the test names start with the case id, for
   example `h3b_a_live_pid_of_another_user_is_alive`), both directions, including 8 concurrent
   takers on a free and on a stale lock. The shell suite `skill/loops/tests/test-locks.sh` runs the
   same cases through a test driver (cases H1 to H11).

## Re-entrancy: a tool under the lock


An agent wrapped a project tool in `crewboard lock run <dir> -- <tool>`. The tool took the
same lock dir itself and waited 26 minutes for the lock that its own parent held. A lock holder
is a process, so its descendants can be told that the lock is held for them.

1. **The grant.** `run` exports two variables to its command, and to nothing else:
   `CREWBOARD_HOST_LOCK=<lock dir, symlinks resolved>` and
   `CREWBOARD_HOST_LOCK_PID=<the pid in the owner file>`. `take` exports nothing (it cannot
   change the environment of its caller).
2. **Re-entry.** A `take` or `run` is re-entrant when all of these hold: the lock exists; the
   owner file names this host; its pid is alive and is a proper ancestor of the script; both
   variables are set; the pid variable equals the recorded pid; the dir variable and the dir
   argument are the same path after the symlinks are resolved (so `~/x`, `/Users/u/x`, a symlink
   and a `../` spelling are one lock). Then the call exits 0 at once, prints one log line
   ("re-entrant: ..."), writes no second owner file and does not wait for CI. `run` passes the
   same two variables on to its command.
3. **Nested release.** A process that got the lock by re-entry never releases it. A separate
   `release` call that meets the same conditions (an ancestor holds the lock, and the variables
   match) leaves the lock in place, prints one line and exits 0, whatever `--holder` it names.
   Only the process that holds the lock (the pid in the owner file is its own) releases it, when
   its `run` command ends.
4. **Ancestor holder, no grant.** If the recorded pid is a live ancestor on this host but the
   variables are missing or name another lock (for example `take --pid <your shell>` and a
   child of that shell, or a wrapper that cleans the environment), `take` and `run` exit 77
   at once, with no wait. The message starts "held by your own parent process <pid>". A holder
   that is not an ancestor is judged as before (75 after `--wait`), whatever the environment says.
5. **Contract for project tools.** A tool that takes the lock itself must not be wrapped in
   `run` ([guide](../../guides/ci-and-host-lock.md#a-tool-that-takes-the-lock-itself)). It takes
   the lock with `crewboard lock`, or it treats `CREWBOARD_HOST_LOCK` equal to its own lock dir
   as "already held by my caller".
6. Proof: the re-entrancy tests (`tests/component/lock_reentry.rs`, real process chains, and the
   fake process table in `src/lock/run.rs`) and the re-entrancy cases of `skill/loops/tests/test-locks.sh`.
   Each check was broken by hand once to see its test fail.

## Repeated runs of a Gate


A series Gate ("N clean loaded runs in a row") false-passed when all runs wrote to one log and a
grep counted the last result line, which could belong to an earlier run.
`crewboard repeat --times N [--lock <dir> --holder A] [--log-dir D] -- <cmd>...` does this. The
command runs as argv in `--repo` (default: the current directory), without a shell; a shell line
is `-- bash -c '<line>'`.
A relative `--lock <dir>` is relative to the current directory of the caller, not to `--repo`.

1. Every run writes its own `run-<n>.log`. An old file of that name is removed first.
2. A run is judged by its own log only: the exit code of the command must be 0, the log must
   have at least one result line (`--result-re`, a regular expression per line; without it the
   exit code alone decides), and no result line may report a failure (`--fail-re`, default
   `FAILED` or a failed count that is not 0). A missing result line is red. A bad expression
   exits 64 with the compile error. The syntax is that of the Rust `regex` crate: the POSIX classes
   (`[[:digit:]]`) work, but the extensions of GNU `grep -E` are not guaranteed.
3. `git rev-parse HEAD` is read before and after every run. A run whose SHA differs from the
   first run is not counted (exit 2).
4. The loop stops at the first red run (exit 1). No "best N of M".
5. With `--lock` each run runs under the lock (the `lock run` code, in the same process; the
   command gets the grant variables). Unix only: a build for a non-Unix host refuses `--lock` with `unix-only`.
6. At the end it prints the raw result lines of every run (ready to paste into the PR) and the
   verdict. The dev role file of the skill (`skill/references/dev.md`) tells the agent to use it.
7. **Signals.** A TERM, INT or HUP to `repeat` stops it: the command (in its own process group) gets
   the signal, the lock is released, the evidence so far is printed, the exit code is 143. The
   same holds while it waits for the lock or for CI.
8. **Exit codes.** They are in the [exit codes reference](../../reference/exit-codes.md#by-command).
9. Proof: `tests/repeat_cli.rs` and `src/repeat/verdict.rs`; cases G1 to G10e of
   `skill/loops/tests/test-locks.sh` run through a test driver.

## CI first, empty lock grace, named reasons


An agent must not take over a lock that a CI job waits for. A directory that looks "stale and
empty" may belong to a taker that has just started, and a takeover would run the CI job beside
the agent's load.

1. **Empty lock.** A lock directory without an owner file and without any other file is stale only when the directory is
   older than the grace time (`--empty-grace`, `$HOST_LOCK_EMPTY_GRACE`, default 30 s). A fresh
   empty directory is a taker between `mkdir` and the owner file and is never taken over. The same
   rule holds in `crewboard wake` (`--empty-grace`, `$CREWBOARD_EMPTY_GRACE`, no pid file).
2. **Named reasons.** A takeover has one of: `pid-dead`, `too-old`, `board-dead`, `board-left`,
   `empty-after-grace`. The log line is `took over stale lock <dir>: old owner <o>; reason=<name>
   (<text>)`. There is no other path.
3. **CI first.** An agent `take` (also a takeover) waits (`--wait`) or exits 76 while a CI job runs
   or waits on the host. CI is seen as a `Runner.Worker` process (`$HOST_LOCK_PS_CMD` replaces
   `ps -axo command=` in tests) or as a file in `<lock-dir>.waiting/<name>` (entries older than
   `--ci-max-age`, default 6 h, are ignored). `ci-wait` and `ci-done` register and leave;
   `ci-status` exits 0 if CI is present. `take --ci` and `run --ci` mark a CI taker: it ignores CI
   presence, is on the list while it waits, and is blocked only by a held lock. A running agent
   job is not preempted.
4. **repeat.** Before each run `crewboard repeat` checks CI like `ci-status`. It pauses with a log line and
   goes on when CI is gone (`--ci-poll`, default 10 s). After `--ci-max-wait` (default 3600 s)
   it stops with exit 3. The evidence block notes each pause. `--no-ci-check` turns it off.
5. Proof: tests H11 to H16 (`src/lock/`, `tests/component/lock_ci.rs`), and cases H11 to H16 and G9
   of `skill/loops/tests/test-locks.sh`, P5c and P5d of `test-loops.sh`.

## Expect and overdue: a lock without a provable holder


An agent took the lock with `take --pid 0` for a four-minute measurement. Forty minutes later
the lock was still held. With `pid=0` no taker can prove the holder dead, so the lock blocked until
`--max-age` (4 h). The board check did not help, because it ran with the default config dir.

1. **`--expect <duration>`** (take, run) writes the line `expect=<seconds>` in the owner file.
   `take --pid 0` without `--expect` exits 64 and the message names `run` as the better form.
   `--pid <number>` and `run` need no `--expect`.
2. **Overdue.** If the owner file has `expect=` and its pid cannot be judged (`pid=0`, no pid, or
   a pid of another host), the lock is stale when it is older than `expect x 2 + 5 min`. The
   reason is `overdue`. A judged pid (this host) is never `overdue`: `pid-dead` decides there. An
   owner file without `expect=` keeps the old rules (`pid-dead`, `too-old`, board).
3. **Notes.** `status` and the wait message add a bracket after the owner. For `pid=0`:
   `pid=0: a dead holder cannot be detected`. Once the lock is older than `expect`:
   `overdue by <n> min` (rounded up, with the expected time and the stale limit).
4. **Durations.** Every time option (`--expect --wait --poll --max-age --empty-grace
   --ci-max-age`) takes whole seconds or a number with one unit: `90`, `90s`, `30m`, `2h`
   (`--poll` also takes a decimal number of seconds). Anything else exits 64.
5. **Config dir of the board check.** The check reads the board of the agent config in the global
   `--config-dir <dir>` (also accepted after the subcommand: `crewboard lock take <dir> --config-dir
   <cfg> ...`) or in `$CREWBOARD_CONFIG_DIR` (the option wins), else in the default config dir. An
   agent wrapper only has to export the variable. If no board config is found and no config dir was
   given, the check is *unavailable*, not failed. `status` prints one line `board check
   unavailable: no agent config dir ...`. `take` and `run` print nothing for this case, so a poll
   loop does not repeat it. No state file keeps the "once": one `status` call is one process and
   prints the line once.
6. Proof: tests E1 to E6f (`src/lock/`, `src/ops/lock_cmd.rs`, `tests/component/lock_cli.rs`,
   `tests/component/lock_board.rs`) and cases E1 to E6f of `skill/loops/tests/test-locks.sh`.

## Foreign holder: a lock without an owner file


A project tool or a CI step can make the lock directory with its own `mkdir` and its own info file
(for example `info`: "github-actions e2e run 4711"). There is no `owner` file. Earlier,
host-lock judged such a directory `empty-after-grace` after 30 s and took it over. Two heavy
jobs then ran together.

1. **Definition.** A directory is a foreign holder when it has no non-empty `owner` file and holds
   at least one other entry, or when its `owner` file is not valid
   ([Invalid owner file](#invalid-owner-file-no-agent-or-start-time)). The temp file `owner.tmp.*`
   of a taker does not count. A directory without any such entry is empty, and rule 1 of "CI
   first, empty lock grace" holds for it.
2. **Stale only by two reasons.** A foreign holder is never `empty-after-grace`.
   - `pid-dead`: a regular file in the directory has a line `pid=<n>` (digits, not 0). The first
     such file in sorted name order (byte order) counts. An optional line `host=<name>` in the same file limits
     the check: if it names another host, the pid is not judged; no host line means this host.
     The pid is dead when the signal check (`kill(pid, 0)`) says the process does not exist; a
     process of another user is alive.
   - `too-old`: the mtime of the directory is older than `--max-age` (`0` = never). The foreign
     file does not need to be rewritten; the clock is the directory.
   Only `owner` has the full format (agent, host, pid, started_epoch). The board check does not
   apply, because there is no agent name.
3. **Text.** `status`, the wait message and the takeover line say
   `foreign holder: <file>: <first line>`. The first line is read from the first file in sorted
   name order. Control characters are removed and the text is cut to 120 characters. A directory
   entry instead of a file is shown as `foreign holder: <name> (not a regular file)`.
4. **Release.** `release` refuses a foreign holder (exit 3), as for any lock of another owner.
5. Proof: tests F1 to F6b (`src/lock/judge.rs`, `src/lock/snapshot.rs`, `tests/component/lock_cli.rs`)
   and cases F1 to F6b of `skill/loops/tests/test-locks.sh`. `crewboard wake` keeps its own
   lock dir rule (pid file); this change does not touch it.

## Invalid owner file: no agent or start time


On one host the lock dir had one file, `owner`, with the single line `<agent>@<host>`. Something
other than `crewboard lock` wrote it by hand. `status` printed `held: @ pid= since ` and exit 0. No stale
rule could fire: no pid, no `expect=`, no start time, no agent name for the board. The lock blocked
every taker for 5.5 hours, and a CI job waited its full 20 minutes and then went on without the lock.

1. **Valid owner file.** An `owner` file is valid only when it is a regular, non-empty file with a
   non-empty `agent=` line (spaces only count as empty) and an all-digit `started_epoch=` line.
   `crewboard lock` always writes both. An empty `owner` file (0 bytes) counts as no owner file, as
   before.
2. **Invalid owner file.** Any other `owner` entry (also a directory named `owner`) is judged as a
   [foreign holder](#foreign-holder-a-lock-without-an-owner-file), with two changes:
   - `pid-dead`: the `owner` file is the first file of the `pid=<n>` search. Its own `host=` line
     limits the check, as for a foreign file.
   - `too-old`: the clock is the mtime of the `owner` file, not of the directory (`--max-age`,
     `0` = never). The mtime comes from the file system metadata (no `stat` parsing, so the GNU and
     BSD spellings do not matter).
   The board, `overdue` and `empty-after-grace` rules do not apply.
3. **Text.** `status`, the wait message, the release refusal and the takeover line say
   `foreign holder: owner (<problem>): <first line>`. The problem is `no key=value fields` (no line
   of the form `name=value`), `no agent= value`, `no numeric started_epoch=`, or both of the last two.
   An `owner` directory is `foreign holder: owner (not a regular file)`. The first line has no
   control characters and has 120 characters at most. Example:
   `held: foreign holder: owner (no key=value fields): agent-a@host-b`.
4. **Release.** `release` never removes a lock with an invalid owner file (exit 3), also when the file
   names the same agent and host. Only a stale takeover removes it.
5. **Valid files keep their rules.** For a valid owner file the clock is `started_epoch`, never the
   file mtime, and all rules of the sections above hold.
6. Proof: tests O1 to O6 (`src/lock/judge.rs`, `src/lock/owner.rs`, `tests/component/lock_cli.rs`) and
   cases O1 to O6 of `skill/loops/tests/test-locks.sh` (both directions). Each check was broken by
   hand once to see its test fail.

## Detached jobs, atomic release, Gate jobs


A Gate command needed 55 minutes; the wake budget (45 minutes) stopped the model and killed the
command with it, and the next wake started it again. A waiter also found a lock dir without an owner file
after such a stop and took it over 35 s later (`empty-after-grace`).

1. **`run --detach --result <file>`.** The caller waits for and takes the lock as `run` does
   (`--wait`, CI first, stale takeover), removes the old result file, and starts the job in its own
   session. It returns at once with exit 0 and prints `detached: pid=<job pid> log=<file>
   result=<file>`. The job output goes to `<file>.log`. `--detach` needs `--result` and `run`.
2. **The supervisor.** The new session starts `crewboard lock` again (the same binary and arguments,
   `HOST_LOCK_SUPERVISE=<caller pid>`; the supervisor). It writes its
   own pid and a line `detached=1` in the owner file, runs the command, and writes
   `rc=<n> end=<utc> log=<path>` to the result file (temp file, then rename). The command runs in
   its own process group. A job that ends by a signal has `rc=128+<signal>`. The supervisor releases
   the lock after the result is written; a release that fails is logged in `<file>.log`. TERM or
   INT to the supervisor goes on to the whole process group of the command. So the stale rules
   (`pid-dead`, `too-old`, board) judge the job by the supervisor pid, which lives as long as the job.
3. **New session.** The supervisor starts with `setsid` (a system call, so there is no `perl` or
   `set -m` branch and no dependence on a `setsid` binary). The group kill of a wake budget stop
   does not reach the job. The caller must not be inside a lock it holds by re-entry (exit 64).
4. **Budget stop.** `crewboard wake` sends TERM to the wake group and waits (at most 10 s) while any
   process of the group lives, so that a `run` in it releases its lock. Then it releases the
   `--host-lock` dirs of `--agent`, but not a lock whose owner file says `detached=1` or whose pid
   is still alive: that lock belongs to a job, not to the wake.
5. **Release is atomic.** `release` (and the supervisor at the end of a job) renames the lock dir to a unique name (`<dir>.rel.*`), then
   removes it. Nobody sees an empty lock dir during a release. The `empty-after-grace` rule stays for
   a taker that crashed between `mkdir` and the owner file.
6. **`--gate`** (take, run): this job runs the Gate of a ticket. It skips the CI-first wait for a
   running or waiting CI job. Every other
   rule stays; a held lock is still waited for. Plain `run` keeps the CI-first order. The flag is
   the switch: the command does not read the `ci_mode` of the initiative.
7. **`--ticket <n>` and `--sha <head>`** (with `--detach`): the result file gets the extra lines
   `ticket=<n>` and `sha=<head>` after the `rc=` line. Both are optional; a result file without
   them is valid.
8. **The wake reports a finished job once** (`crewboard wake`, `--events-file`). The result files
   of an agent are in `<state dir>/results/` (the wake gets the dir as `CREWBOARD_RESULTS_DIR`).
   A file with an `rc=` line that is not reported yet adds the line `detached job finished: ticket
   <n> rc=<rc> log=<log> result=<file>` to the events file and wakes the agent although the board
   is quiet. `<file>.seen` holds the reported `rc=` line, so a poll does not report it again; a
   new run of the job writes a new line and is reported again. A file without an `rc=` line, a file
   outside the dir and a poll without `--events-file` are not read. So `done` runs once after the
   wake that sees the event, with no cron job of the agent.
9. Proof: tests DT1 to DT7 and GT1 to GT4 (`tests/component/lock_detach.rs`, `lock_ci.rs`), cases DT1
   to DT4, DT6 and GT1 to GT4 of `skill/loops/tests/test-locks.sh` (DT5 and DT7 live in Rust only:
   the shell proof of them needed stubs for `rm` and `setsid`, which the binary does not call), and
   cases RS1 to RS3 of `skill/loops/tests/test-loops.sh`.
