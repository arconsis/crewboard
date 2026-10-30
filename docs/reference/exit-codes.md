# Exit codes

This page is the one list of the exit codes of `crewboard`. The code is `src/exit.rs` plus the
commands that return their own code (`lock`, `repeat`, `wake`). Other pages link here and keep
no table of their own. The [CLI reference](cli.md) lists the commands and options.

## One meaning for each code

| Code | Name | Meaning |
|---|---|---|
| 0 | ok | Success. For `watch` and `poll`: events were printed. For `mine`: work to do now. `mine <n>` returns 0 when it prints the thread. |
| 1 | no | The answer is no and nothing is wrong. See the [commands that return 1](#commands-that-return-1). |
| 2 | refused | A rule refused the command. The message names the rule and the fix. |
| 3 | external | GitHub, CI or the network did not deliver. |
| 4 | config | Configuration, key or env file error. A person fixes the setup. |
| 64 | usage | Usage error: a bad option, a missing or bad argument (for example an issue number), a bad combination of options. Fix the command and run it again. |

Most commands return only 0, 2, 3, 4 and 64. A watcher that cannot see events never returns 1: it
returns 3 or 4. So a quiet board and a broken watcher never look alike. Every "no events" result
and every error of `watch` and `poll` also prints one line with the cursor, the number of polls
and the time of the last successful poll.

A usage error (a bad option, a missing argument) is 64 for every command, also for `lock`,
`repeat` and `wake`. It is not 4: a usage error is a typo to fix, not a setup that stops the
agent. `--help` and `--version` return 0.

## Commands that return 1

| Command | 1 means |
|---|---|
| `watch`, `poll` | No events: timeout, or nothing new. `poll --quiet` returns 0 (events), 1 (none) or 2 or more (an error). |
| `mine` | Nothing to do now: every owned ticket waits, or the agent owns none. A `hint:` line never changes the code. |
| `lock status` | The lock is free. |
| `lock ci-status` | No CI job runs or waits. |
| `repeat` | A red run. |

`monitor` never returns 1. It returns 0 after events (or after `--max-cycles` silent cycles). On
an error it prints one line on standard output and returns the code of that error (2, 3, 4, or
64 for a bad option).

Four commands return 1 for a fault or a skip. These are exceptions, not the "no" answer:

| Command | 1 means |
|---|---|
| `lock take`, `lock run` | The lock directory cannot be made, or its owner file cannot be written. The lock is not taken. |
| `lock ci-wait` | The waiting list cannot be written. |
| `skill install` | A step was skipped, or an existing real directory or foreign link was left alone. |
| `update` | The binary is updated, but `skill install` returned 1. |

## Codes of the host tools

`lock`, `repeat` and `wake` run on a host, next to schedulers and workflow scripts. They add
these codes. Their usage errors are 64 too: a bad option, a bad number, a bad regular
expression, no command.

| Code | Name | Meaning | Commands |
|---|---|---|---|
| 70 | wake-failed | The wake command failed. | `wake` |
| 71 | wake-lock | The wake lock cannot be taken. | `wake` |
| 75 | lock-held | A live owner holds the lock, after `--wait`. | `lock take`, `lock run` |
| 76 | ci-active | A CI job runs or waits, after `--wait`. Agents only. | `lock take`, `lock run` |
| 77 | lock-ancestor | Your own ancestor process holds the lock and gave no grant. No wait. | `lock take`, `lock run` |
| 126, 127 | cannot-start | The command could not start: 127 not found, 126 not allowed or another start error. | `lock run`, `agent exec`, `agent with-env` |
| 128 + N | signal | Stopped by signal N. `lock run` and `repeat` return 143 after TERM, INT or HUP. | `wake`, `lock run`, `repeat` |

On a build without Unix process control, `lock`, `wake` and `repeat --lock` refuse with the rule
`unix-only` (exit 2).

## By command

| Command | Codes |
|---|---|
| `lock take`, `lock run` | 0 got the lock (`run`: the code of the command), 1 [fault](#commands-that-return-1), 64, 75, 76, 77, 126, 127, 143 |
| `lock release` | 0 released, nothing to release, or a nested release. 3 not the owner: the lock stays. |
| `lock status` | 0 held by a live owner, 1 free, 2 stale (the line names the reason). |
| `lock break --if-dead` | 0 free, or stale and removed. 3 held by a live owner. 64 without `--if-dead`. |
| `lock ci-wait`, `lock ci-done` | 0. `ci-wait` returns 1 when the waiting list cannot be written. |
| `lock ci-status` | 0 a CI job runs or waits, 1 none. |
| `repeat` | 0 all N runs green on one commit. 1 a red run (the loop stops there). 2 the head commit changed, so the run is not counted. 3 CI did not finish within `--ci-max-wait`, so the command did not start. 4 setup error: `--repo` is not a git checkout with a commit, or the logs cannot be written. 64, 143. A command that does not start is a red run: the loop ends with 1. |
| `wake` | 0 nothing to do, woke OK, a wake already runs, the budget ended it, or the wakes are stopped. 2, 3, 4 from the board calls. 64, 70, 71, 128 + N. |
| `wake install`, `wake uninstall`, `wake status` | 0, 2 (another wake job runs this agent: `--replace` removes it), 4 (a failed scheduler command), 64 (a wrong option). |
| `agent add` | 0, 2, 3, 4, 64. |
| `agent exec`, `agent with-env` | 126 or 127 when the command does not start, 4 for a config error, 64 for a usage error (`with-env` without `--env-file`). After a start the command replaces the process. |
| `skill install` | 0, 1 (see above), 3 a failed step, 4 for `--link` without a clone. |
| `update` | 0 (also "already installed"). 2 refused: no release for this platform, or a checksum, tarball or version check failed. 3 a failed step (a write or the download). 4 a token that GitHub refuses. 64 usage. |
| `doctor --setup` | 0, or 2 when a check fails. |
| `hook post-tool-use` | Always 0. A failure is a line in the log. |
| every other command | 0, 2, 3, 4, 64. |

## Three exceptions that scripts rely on

`lock status` returns 2 for "stale", and a script decides on `lock break` by it. `lock release`
returns 3 for "not the owner". `lock break --if-dead` returns 3 for "held by a live owner". These
codes keep their numbers because workflow scripts read them.

## Where the specification uses these codes

- [CLI: exit codes](../design/spec/cli.md#exit-codes)
- [Host lock](../design/spec/host-lock.md)
- [Events](../design/spec/events.md) for the retry of exit 3 and the stop on exit 4
