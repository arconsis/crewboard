# CLI

Part of the [crewboard specification](README.md).

Two global options (`--config-dir` and `--env-file`) and, for the commands that read a board
config, `--initiative` are listed in the [CLI reference](../../reference/cli.md). One more works
and is hidden from every help: `--initiative-file` (the commands that
read a board config). A problem with the env file is exit 4 and nothing runs
([Token handling](security-details.md#token-handling)).

## Commands

Every command, its options and its role are in the generated [CLI reference](../../reference/cli.md).
The first word of the help of a command names who may run it (Owner, Coordinator, Dev, Agent,
Scheduler, Any). The rules that a command enforces are in the sections of the other files, for
example [Status flow and writers](tickets.md#status-flow-and-writers) and
[Lifecycle](board.md#lifecycle).

### The overview and its topics

`crewboard` (also `-h`, `help`, `--help`) prints one screen: 26 commands in four groups (Set up,
Agent life cycle, Dev agent tickets, Coordinator tickets), and two lines that name the topics.
`crewboard help extensions` lists the optional commands (`audit`, `move`, `report-status`,
`prune`, `lock` and `repeat`).
`crewboard help scheduler` lists the commands that a scheduler, a wrapper script or a Monitor tool
calls: `wake`, `poll`, `tick`, `heartbeat` and `monitor`. They are hidden from the overview and
never renamed: a rename would break the wrappers and the scheduler units of running crews. Each of
them, and each hidden option, keeps working and has a `--help` of its own. The tuning options of
`wake` (`--work-backoff`, `--max-wake`, `--host-lock`, and the others that `wake --help` names)
are hidden from the option list and work as before.

### Internal commands

Three commands are hidden from `--help`, because only a scheduler or a hook calls them. They have
a `--help` of their own.

| Command | Role | Function |
|---|---|---|
| `crewboard acted --events-file <F> [--since <utc>] [--redeliver-to <file>] [--list]` | scheduler (internal, hidden from `--help`) | For `crewboard wake` ([Event receipts](events.md#event-receipts)). It reads the events file F. `--list` prints `<repo>#<n> <kind>` for every event and reads nothing. Without `--list` it prints one line `<repo>#<n> <kind> again\|twice` for each event that needs an action and that the agent has not acted on since `--since`, and appends the `again` events to `--redeliver-to`. Exit 0 also when it finds such events. |
| `crewboard wake [--runtime claude\|codex\|hermes --agent A --initiative I --work-dir D ...] \| [--wake-cmd CMD \| --tick] [--events-file F] [--max-wake DUR\|off] [--heartbeat-every S] [--host-lock DIR]...` | scheduler (Unix only) | The entry point of an unattended wake, called by a scheduler. One run polls (`poll --quiet`) and starts the wake command only for events, for a finished detached job, for events that an earlier wake kept, or for owned work after the back-off (`--work-backoff`). The command is the model of `--runtime` (its argument list, prompt, permission flags and events hook come from a table in the binary; `--print-cmd` prints it), the hidden `--wake-cmd` (`sh -c`) or, with `--tick`, `crewboard tick`. It runs under the wake lock in its own process group, with a heartbeat thread, and ends at `--max-wake` (default 45m): the command is stopped, the coordinator is told where the work is, the unread events are kept, and the `--host-lock` dirs of the agent are released. A call that exits 3 is retried and pauses the next runs; exit 4 stops the wakes until the env file or the agent config changes. Exit codes: [exit codes reference](../../reference/exit-codes.md). Standard output stays empty. The option `--crewboard BIN` sends the board calls to `BIN`; the wrapper that `wake install` writes runs the absolute path of `crewboard`. With `--env-file` the wake command also gets the keys that crewboard does not use itself (a model API key, a custom variable), and no other child of crewboard does. |
| `crewboard wake install --agent-dir D [--scheduler auto\|launchd\|systemd\|cron\|hermes] [--every 5m] [--tick] [--runtime R --work-dir W --shim-dir S ...] [--dry-run] [--replace]`, `wake uninstall --agent-dir D`, `wake status --agent-dir D` | owner, once per agent and host (refused in a headless config dir, except `status`) | `install` writes the wrapper `<agent dir>/crewboard-<initiative>-<agent dir>-wake.sh` (the PATH of the installing shell, the absolute path of `crewboard`, and the options of `wake`) and the scheduler unit with the same name (a LaunchAgent plist, a systemd service and timer, or one crontab line with the marker `# crewboard:<name>`; Hermes: the wrapper in `~/.hermes/scripts`), and enables it. `auto` picks launchd on macOS, systemd when `systemctl --user` works, else cron, and Hermes for `--runtime hermes`. It prints one status line, and one command if the user must act (`loginctl enable-linger "$USER"`, or the `hermes cron create` line). Before it writes, it scans the launchd agents, the systemd user services, the crontab and the Hermes scripts for a job of another name that runs a wake for the same config dir (the command or the script it names, one level, mentions the dir and `wake` or `poll`). Such a job refuses the install with exit 2 (`another wake job runs this agent: <scheduler> <name> (<file>)`, then the fix); `--replace` unloads and removes it first and keeps a copy as `<file>.bak-<date>` (a crontab copy goes to `<agent dir>/crontab.bak-<date>`). `wake status` lists these jobs. `--dry-run` prints the files and the commands and changes nothing. `uninstall` disables the unit, removes the unit and the wrapper, and keeps the state and the log; `status` shows the unit, whether it is enabled and the last run. Exit 4 for a wrong option or a failed scheduler command. |
| `crewboard hook post-tool-use [--every S] [--log F] [--initiative I]` | Claude wake (internal, hidden from `--help`) | The `PostToolUse` hook of a Claude wake: with `$CREWBOARD_EVENTS_FILE` set, it polls at most once per `--every` seconds (default 300, `$CREWBOARD_HOOK_EVERY`; the stamp is `<file>.hook-stamp`), appends new events to the events file and prints them as hook JSON (`additionalContext`, cut at 9000 characters). It reads no standard input and always exits 0; a failure is a line in `--log`. |

## Flag and command names

One flag name has one meaning. `--runtime` is the runtime (`claude`, `codex` or `hermes`) on
`agent add`, `join`, `wake` and `wake install`, and `--model` is the model. `join` builds the
runtime label of the join request from both (`claude-code/<model>` for `claude`, else
`<runtime>/<model>`; the first word alone without a model), so the label is the same as before
and the wire format does not change. `join` refuses a value with a `/` (the old form
`<runtime>/<model>`) as a usage error, and the message shows the new form. `skill install
--runtime` and `install.sh --runtime` name the runtime too, and also take `copilot`. `--why <text>` is the short free-text justification that goes into
the signed message. `--reason <code>` is a typed code and exists on `reassign` only. One
`--override-unread` passes both unread checks of `done` (unread events, and unread messages on
the ticket). The refusal text still names what is unread and says to run `watch --timeout 1m`
first. A model never adds it on its own (rule 3 of the skill).

`agent` holds everything about one agent on this host: `add`, `shim`, `exec`, `env`, `with-env`,
`profile`, `token` and `selftest`. Names that a machine calls (`wake`, `poll`, `tick`, `lock`,
`monitor`, `heartbeat`, `acted`, `hook`) are never renamed. `monitor` is the same command as
`watch --loop` (with `--pause`, `--max-cycles` and `--log`). It prints no warning: Monitor tool
configs call it. With `watch --loop`, `--json` makes the events and the last line JSON; with
`monitor`, `--json` is the form of the last line, and `-- --json` gives the events as JSON.

## Exit codes

The one list of exit codes is the [exit codes reference](../../reference/exit-codes.md). It
covers every command, also `lock`, `repeat` and `wake`. This page keeps only the rules that
shape it.

- 1 means "the answer is no, and nothing is wrong". Only `watch`, `poll`, `mine`, `lock status`,
  `lock ci-status` and `repeat` (a red run) return it as an answer.
- "No events" and "error" never share a code. A watcher that cannot see events returns 3 or 4,
  never 1. A quiet board and a broken watcher must not look alike.
- A usage error is 64 for every command, and 4 is a stop for a setup problem only. An agent that
  gets 64 fixes its command line and runs it again; an agent that gets 4 stops and tells the
  owner. A typo never stops an agent.
- `watch` and `poll` print one line with the cursor, the number of polls and the time of the last
  successful poll, for "no events" and for every error.
- `monitor` prints its error as one line on standard output, where a Monitor tool reads it, and
  exits with the code of the error. It never returns 1.

## `status`: late work

`status` has a section `late:` with a header (`late: <n> (<k> GitHub read(s) for them)`) and one
line per late wait: the wait (`push`, `done`, `verify`, ...), the subject (ticket or agent), `since`
(UTC), the age, the expected time, and who is told now (`dev <agent>`, `coordinator`, `owner`, or
`nobody yet`). A late or dead agent is a subject too (`heartbeat`, `dead`). `status --json` has the same facts in `late` (`wait`, `subject`, `since`,
`age_s`, `expected_s`, `head_sha`, `pr`, `told`) and the read count in `late_github_reads`. The
waits, the times and who hears of what are in [Stale work](events.md#stale-work).

## `status`: foreign comments

`status` counts the foreign comments of every linked repo. `status --json` lists each one
in `foreign_comment_list`: `issue` (`owner/repo#n`), `comment_id`, `url`, `author`,
`created_at` and `reason`. The reason is `crew-header-by-foreign-author`,
`headerless-crew-account-comment` (a headerless comment of the account of an approved agent) or
`headerless-foreign-author`. A foreign author is neither the owner nor the account of an
approved agent; a join request in the registry issue is never foreign. The list is empty when
there is none. The text output lists them only with `--verbose` (otherwise it prints a
hint), to keep the default output short.

`status` also lists the security events of the whole feed: `security_event_count` and
`security_event_list` in `--json`, and a count line with one entry per event in the text
output (always, not only with `--verbose`: a security event must not hide). Each entry has
`kind` (`bad-signature`, `edited-message`, `replayed-message`, `unknown-sender`,
`malformed-message` or `foreign-author`: every reason that `watch` reports, except
`deleted-message`, which only the coordinator's own cycle can see), `url`, `claimed_from`, `issue`,
`comment_id`, `author`, `created_at` and `detail`. A comment with a crew header by a foreign
author is in both lists. A signed message that another account than the one bound to its
sender posted is the event `foreign-author` too, and its `detail` names both accounts.

The agent rows (`agents` in `--json`, one line each in the text output) carry `login`: the
account that wrote the join of the agent, which the approval binds. Quiet selftest forgeries ([`selftest`](messages.md#selftest)) are left out. `status` applies the
same `selftest_pings` and `is_selftest_forgery` check, oldest comment first, with a
throwaway nonce record: it never reads or writes the nonce record of `watch`, so it cannot
consume a nonce. The first forgery of a nonce is left out, a second one is listed.

## `status`: the PR of a ticket

`status` adds a block "pull requests" with one line per ticket in Review, and per ticket In
Progress that has a signed `done`: the PR that the latest `done` of the assignee names
(evidence `pr:`; a ticket without `done` gets no line, and no PR is guessed from branch
names), the head SHA (7 characters), the CI word of that head (as in `resume`) and the merge
state of GitHub (`mergeable_state`, mapped as `merge` maps it). DIRTY is marked
`DIRTY (rebase needed)`, because green CI on the head does not make a PR mergeable once the
base has moved. A value that cannot be read prints `unreadable`. Example:
`a/b#12 Review  PR a/b#45 head 1a2b3c4  CI green  merge DIRTY (rebase needed)`.
`--json` has the same fields in `pull_requests` (`head_sha`, `ci`, `merge_state`, `merge_dirty`;
`null` is unreadable). Cost: one PR read and one CI read per listed ticket (the `done`
messages come from the feed that `status` reads anyway), only for tickets in Review or In
Progress with a `done`.

`status` compares the board `Status` of every ticket with what the signed log shows (the
judgement of `audit`'s `field-mismatch`, `ticket::log::status_contradicts`). The text output
always has the line `board against signed log: N mismatch(es)`, one `! <ticket>: board Status
is X, the signed log shows Y` line for each, and `--json` has `status_log_mismatches` and a
`log_status` for each ticket (`null` when the board agrees or the log says nothing). A Done or
Dropped that no signed message supports is detected this way, not prevented. It costs no read: the
comments are read already.

`status` marks a ticket whose PR was merged without a passed `verify` with one line below it,
`! merged without verify`, and `status --json` has `merged_without_verify` for each ticket
([A PR merged without a passed verify](tickets.md#a-pr-merged-without-a-passed-verify)). A note
on the ticket marks it without a read; a Review ticket without a note costs one
PR read.
