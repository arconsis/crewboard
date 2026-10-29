//! The text of `crewboard lock --help`.

pub const HELP: &str = r#"crewboard lock - a host lock that a dead agent cannot hold forever.

Agents on a shared host take one lock before a heavy job (full build, stress gate, load
test). The lock is a directory (mkdir is atomic) with an owner file. If the owner dies,
the next taker sees that the lock is STALE and takes it over.

Usage:
  crewboard lock take    <dir> --holder AGENT [--reason TEXT] [--wait S] [--pid P] [--expect D]
  crewboard lock release <dir> --holder AGENT
  crewboard lock status  <dir>
  crewboard lock break   <dir> --if-dead [--log FILE]   (remove the lock only if it is stale)
  crewboard lock run     <dir> --holder AGENT [--reason TEXT] [--wait S] [--gate] -- CMD...
  crewboard lock run     <dir> --holder AGENT --detach --result FILE [--ticket N] [--sha HEAD] [--reason TEXT] [--wait S] [--gate] -- CMD...
  crewboard lock ci-wait <dir> --holder NAME      (a CI step: register in the waiting list)
  crewboard lock ci-done <dir> --holder NAME      (a CI step: leave the waiting list)
  crewboard lock ci-status <dir>                 (exit 0 and a line if CI runs or waits, else 1)

Options (all subcommands that take a lock):
  --holder AGENT   who holds the lock (the agent name, for example claude@host-b). Required.
  --reason TEXT    why (shown to the others), for example "make test".
  --wait D         how long to wait for a live lock (default 0 = do not wait; exit 75).
                   A duration D is whole seconds or a number with a unit: 90, 90s, 30m, 2h.
                   All time options (--wait --poll --max-age --empty-grace --ci-max-age
                   --expect) take this syntax.
  --poll D         time between two tries while waiting (default 5).
  --pid P          the process that holds the lock (take; ci-wait records it in its entry).
                   Default: the parent process of this process (ci-wait: none).
                   0 = no pid check (see "pid" below). It needs --expect (exit 64 without).
                   runner = the nearest `Runner.Worker` ancestor of this process (a GitHub
                   Actions job). It lives exactly as long as the job. Exit 64 if there is none.
  --max-age D      a lock older than D is stale (default 14400 = 4 h, or
                   $HOST_LOCK_MAX_AGE). 0 = no age limit.
  --expect D       how long the job should take (take, run). It goes in the owner file as
                   `expect=<seconds>`. Required with `--pid 0`. A lock whose pid cannot be
                   judged (pid=0, no pid, or a pid of another host) is stale after
                   D x 2 + 5 min, reason `overdue`. An owner file without `expect=` keeps the
                   --max-age rule only.
  --no-board       do not ask the board whether the owner agent is dead.
  --config-dir D   the config dir of the owner agent, for the board check (also the global
                   option `crewboard --config-dir D lock ...`). $CREWBOARD_CONFIG_DIR is its
                   default. An agent whose config is not in the default dir (for example
                   ~/.config/crewboard/agents/<agent-dir>) must set one of them, or the board
                   check cannot run. Without a config dir and without a default config,
                   `status` prints one line "board check unavailable: no agent config dir".
                   take and run stay silent about it, so a poll loop does not repeat it.
  --initiative I   the initiative of the board check, when the config dir has more than one.
  --log F          also append the log lines to F.
  --empty-grace D  an EMPTY lock directory counts as stale only when it is older than D
                   (default 30, or $HOST_LOCK_EMPTY_GRACE). A fresh empty directory is
                   a taker between mkdir and the owner file. A directory with a foreign file
                   is not judged by this (see "Foreign holder").
  --ci             this taker is a CI job: it ignores CI presence and waiting CI entries
                   (it is never blocked by a waiting agent, only by a held lock). While it
                   waits it is on the waiting list itself. For take and run.
  --ci-max-age D   a waiting-list entry older than D is ignored (default 21600).
  --gate           this job is a merge-deciding gate: skip the CI-first wait for a
                   running or waiting CI job. Every other rule stays: a held lock is waited for.
                   The caller chooses it, for example under `ci_mode: local`. For take and run.
  --detach         (run only, needs --result) wait for and take the lock as `run` does, then start
                   CMD in its own session and return at once with exit 0. Stdout:
                   `detached: pid=<job pid> log=<file> result=<file>`. The job holds the lock until
                   it ends, then writes the result file and releases the lock. A kill of the
                   caller (for example the time budget of a wake) neither stops the job nor
                   releases its lock.
  --result FILE    with --detach: one line `rc=<n> end=<utc> log=<path>` appears here when the job
                   ends (written to a temp file, then renamed). Removed at the start of the run.
                   The job output goes to FILE.log.
  --ticket N       with --detach: the result file gets a second line `ticket=N`. The wake
                   wrapper (poll-and-wake.sh) names it in the events file when the job has ended.
  --sha HEAD       with --detach: the result file gets a line `sha=HEAD` (the PR head; optional).

Stale lock: any ONE of these named reasons. There is no other way to take a lock over.
  overdue:           the owner file has `expect=`, the pid cannot be judged (pid=0, no pid, or
                     another host) and the lock is older than expect x 2 + 5 min.
  pid-dead:          the lock was made on THIS host and its pid is gone. A pid that another
                     user owns (the signal check gives EPERM) is alive. On another host the
                     pid says nothing, so it is never judged.
  too-old:           started longer ago than --max-age (any host).
  board-dead / board-left: the board says the owner agent is dead, or its State is
                     left. The check runs in this process (the same answer as `crewboard
                     status`). If the board or the config is not reachable, the
                     check is skipped, and the lock is not judged by the board. The note on
                     stderr names the cause (the first line of the error),
                     once per cause; a working check after a failure gives one line "board
                     check works again". A failure never stops a wait: the pid and age
                     checks go on. The answer is cached for 60 s (also a failed one; set
                     $HOST_LOCK_BOARD_TTL for tests), so a wait polls the board once a minute,
                     not at every poll. An agent that the board does
                     not list is not judged.
  empty-after-grace: the lock directory is EMPTY (no owner file, no other file) AND is older
                     than --empty-grace. A fresh empty directory is NEVER taken over.
Foreign holder: a directory with no owner file but WITH another file is a lock
that a project tool or CI step made with its own mkdir and info file. It is never
`empty-after-grace`. It is stale only by `pid-dead` (a line `pid=<n>` in any regular file of the
directory, an optional line `host=<name>` in the same file; the pid is judged only on this host,
a missing host line means this host, `pid=0` means none) or by `too-old` (--max-age, counted from
the mtime of the directory; --max-age 0 = never). status, the wait message and the takeover line
say `foreign holder: <file>: <first line>` (control characters removed, 120 characters at most).
A tool should write an `owner` file instead (see docs/guides/ci-and-host-lock.md).
Invalid owner file: an `owner` file that is not empty but has no non-empty `agent=`
line or no numeric `started_epoch=` line is NOT a valid owner file (for example one line
`<agent>@<host>` written by hand). It is judged as a foreign holder, with two changes: the
owner file is the first file of the pid search, and `too-old` counts from the mtime of the
owner file (not of the directory). No board, overdue or empty-after-grace rule applies. The
text is `foreign holder: owner (<what is wrong>): <first line>`, for example
`foreign holder: owner (no key=value fields): agent-a@host-b`. An empty `owner` file (0 bytes)
counts as no owner file, as before.
CI first: an AGENT take (also a takeover) waits, or exits 76 without --wait, while
a CI job runs or waits on this host. CI is seen in two ways: a process `Runner.Worker` (the
GitHub Actions runner; $HOST_LOCK_PS_CMD replaces `ps -axo command=` for tests), and a file
in the waiting list `<dir>.waiting/<name>`. A CI step registers with `ci-wait` BEFORE it waits
for the lock and leaves with `ci-done` (use `if: always()`); `take --ci`/`run --ci` do both
by themselves while they wait. A CI take is blocked only by a held lock.
A stale lock is taken over atomically: only one taker passes the takeover guard
(`<dir>.takeover`, a mkdir), re-checks that the lock is still stale, renames it to a unique
name (mv) and then makes its own. The takeover prints ONE log line with the old owner and
the reason. Nobody does rm-then-mkdir on the live name.

pid: `take` records a pid so that others can see a dead owner. Each Bash call of an agent
is a short-lived shell, so its parent pid is gone when the call ends. For take and
release in two separate calls use `--pid 0 --expect <d>` (the age, overdue and board checks
still work; a dead holder cannot be detected), or better use `run`, which holds the lock for exactly as long as its own process lives.
In CI use `--pid runner` for `take` and `ci-wait`: Runner.Worker lives as long
as the job, so a job that dies frees its lock and its waiting entry (same host only).
The walk uses `ps -o ppid= -p P` and `ps -o comm= -p P`. For tests, $HOST_LOCK_PARENT_CMD
replaces both: it is run as `sh -c "$HOST_LOCK_PARENT_CMD" sh P` and prints "PPID COMM".
A waiting entry (`<dir>.waiting/<name>`, key=value lines) has a `pid=` line when a pid is
known. An entry whose pid is dead on its own host is ignored and removed. An entry without
a `pid=` line (an older version wrote it) is judged by its age only.

Release removes the lock only if the owner file is valid and names the same agent and host.
It is atomic: the lock dir is renamed to a unique name first and removed after, so no
other process sees an empty lock dir during a release.
Anyone else gets exit 3 and the lock stays. An invalid owner file is never released, whatever
agent and host it names: only a stale takeover removes it.

Owner file (`<dir>/owner`, key=value lines): agent, host, pid, started (UTC),
started_epoch, reason, expect (seconds; only when --expect was given), detached=1 (a detached job;
its pid is the pid of the job supervisor, which lives as long as the job). It is valid only with a
non-empty `agent=` and an all-digit `started_epoch=` (see "Invalid owner file").

Re-entrancy: `run` exports CREWBOARD_HOST_LOCK=<the lock dir, resolved with
`pwd -P`> and CREWBOARD_HOST_LOCK_PID=<the holder pid> to CMD (not set anywhere else). A
`take` or `run` for that dir, in a process whose ANCESTOR is the recorded holder (same host),
and that carries both variables with the same dir and pid, gets the lock at once. It prints
one line, writes no second owner file, and its `release` leaves the outer lock alone. An
ancestor holder WITHOUT that grant (for example a `take --pid <your shell>`, or an env that a
wrapper stripped) is not re-entered: take/run exit 77 at once, with no wait, and say "held by
your own parent process". A tool that takes the lock itself must not be wrapped in `run`.

Exit codes: take/run: 0 got the lock (run: the exit code of CMD), 75 held by a live owner
(and --wait over), 76 CI runs or waits (agents only, and --wait over), 77 held by your own
ancestor process and no re-entry (no wait), 64 usage. release: 0 released or nothing to release
(also a nested release, which leaves the outer lock), 3 not the owner.
status: 0 held by a live owner, 1 free, 2 stale (it prints the reason). A held line ends with
alive=yes|no|unknown (kill -0, only for a pid on this host) and age=<s>.
break: 0 free, or stale and removed; 3 held by a live owner (refused, lock untouched); 64 no --if-dead.
Unix only (macOS, Linux). 
"#;
