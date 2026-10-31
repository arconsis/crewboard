# CI

Part of the [crewboard specification](README.md).

## CI proof

One question: what proves CI for the head SHA of a PR? There are four answers. Each is a *kind of
CI proof*. The other pages link here and keep no list of their own.

| Kind | How it arises | Line in the signed `done` | A headless `tick` may merge |
|---|---|---|---|
| `runs` | The default. The counted runs of the head SHA are green ([Which CI runs count](#which-ci-runs-count)). `--run` adds named runs to the evidence. A red run is never hidden. | `ci: <run url>` and `ci-runs: ...` | Yes, if the Gate is CI only and the evidence names no run by hand |
| `none` | The ticket body says `CI: none`. The assignment signs it as `ci: none`. | `ci: none` | No |
| `skipped` | `done` finds no run, and every workflow ignores all changed files ([Files that every workflow ignores](#files-that-every-workflow-ignores)). Code judges this. | `ci: none (all files paths-ignored: <patterns>)` | No |
| `local` | The owner sets `ci_mode: local`, and `done` carries `--local-run` ([Local run](#local-run-ci_mode-local)). | `ci: local-gate` and `local-gate: <text>` | No |

`--run` is not a kind. It adds named runs to `runs`. A ticket that is not `CI: none` gets
`ci: required` in its assignment. In `ci_mode: ci`, `done` then needs `runs` or `skipped`. In
`ci_mode: local` it needs `local`.

Only `runs` can merge without a person. The headless `merge` and `tick` also need a Gate that is
CI only ([Headless coordinator](agents.md#headless-coordinator)). The three other kinds are a
judgment of the interactive coordinator.

The wire strings `ci: local-gate` and `local-gate:` keep their names; the flag is `--local-run`.
The prose calls the check a *local run*, because "Gate" means one thing: the success line of a
ticket ([Concepts](../../concepts.md#glossary)).

## CI must run for crew branches

`done` needs a CI run for the head of the PR ([Evidence](tickets.md#evidence)). A repo can run CI only for pushes to
`main` and `feat/**`, with no `pull_request` trigger. Then no crew PR can reach Review. A
second trap: for a push, GitHub uses the workflow file of the pushed commit, so a crew branch
made before a CI change gets no run until it merges the base branch.

`init`, `start` and `agent selftest` read the workflows of every repo of the initiative
(`.github/workflows/*.yml` through the contents API) and warn when no workflow runs on a push to
`<dev_branch_prefix>**` (a `push` event with no `branches` filter, or a filter that matches) or on
`pull_request` to the base branch. The reader covers `on: push`, `on: [push, pull_request]`,
a block list, `on: {push: {branches: [...]}}`, the block form, `branches-ignore`, `!` patterns,
and the globs `*` and `**`. A form that it cannot read gives the warning "cannot confirm", never a
sure "no trigger". A read error is a warning too; the check never stops `init` or `start`.

The `done` refusal `evidence-ci-missing` (and the `verify` failure) names the likely cause: no
workflow ran for the SHA; check the triggers for `<dev_branch_prefix>**`; merge the base branch
into a branch that was made before the CI change.

## CI and crewboard-only commits


A merge that changes only `.crewboard.toml` or `AUTHORITY.md` starts the full CI of the repo if a
workflow runs on push to the base branch without a filter for these files. On a scarce self-hosted runner this can take
about 15 minutes and delay the verdicts of dev PRs.

1. **The owner's step.** The owner adds `.crewboard.toml`, `AUTHORITY.md` and the crewboard docs
   folder (if one is configured) to `paths-ignore` of the push trigger, next to the docs paths.
   The workflow change is the owner's: `ci-config` is owner-only and the owner route ([Workflow files: the owner route](tickets.md#workflow-files-the-owner-route))
   delivers it as a patch. crewboard never edits workflows. docs/guides/owner.md shows the YAML.
2. **The warning.** `init`, `start` and `agent selftest` check with the reader of [CI must run for crew branches](#ci-must-run-for-crew-branches) every
   workflow of every repo. A workflow counts if its `push` trigger runs for the base branch.
   For each of the two files, the workflow runs for a change of only that file unless: its
   `paths-ignore` selects the file, or its `paths` allow-list does not select it. Globs: `*`
   (not `/`), `**` (also `**/` for zero directories), a leading `!` removes a match (the last
   match decides, as for branches).
3. **The text.** The warning names the repo, the workflow and the files that start it. Partial
   cover names only the missing file. A workflow that does not run on push to the base branch
   gives no warning. A form that the reader cannot read gives "cannot confirm", never a sure
   warning, and a sure warning of a readable file is not hidden by an unreadable one. A read
   error is a warning.
4. **Never a refusal.** The check is a hint. `start` is never refused for it.

## Files that every workflow ignores


An example: the CI ran on `push` to `crew/**` with `paths-ignore: docs/**, README.md, ...`. A
docs-only crew PR therefore had no CI run at all, and `done` refused with `evidence-ci-missing`.
The coordinator then added `CI: none` to the ticket by hand, 10 to 15 minutes each time, three
times in one day. crewboard can see this case for itself.

1. **When.** Only when `done` finds no workflow run, check run or commit status for the head
   (`CiFacts::NoCi` in `src/rules/evidence.rs`), and the ticket does not declare `CI: none`.
   A head with any run (also an information run, [Which CI runs count](#which-ci-runs-count))
   is judged as before. The judgement never turns a red or a pending CI green.
2. **Accept only if ALL of these hold** (`judge_no_run` in `src/services/ci_triggers.rs`):
   - every CI source could be read (an unreadable `check-runs` or `statuses` source can hide a
     run of an external CI);
   - the PR has at most 300 files (GitHub judges `paths` filters on at most 300 changed files);
     the old paths of renamed files count as changed files too;
   - the reader understood every workflow file at the head SHA (no "cannot confirm"), and no
     workflow has an event that is not in GitHub's event list;
   - at least one workflow runs for the PR by its event and branch filter (`push` with the PR
     branch; `pull_request` and `pull_request_target` with the base branch). Without one, CI is
     missing, not skipped;
   - every such workflow has a `paths` or `paths-ignore` filter that excludes every changed
     file. A workflow with both `paths` and `paths-ignore` (or `branches` and
     `branches-ignore`) for one event, or a pattern that the reader cannot read, refuses;
   - the base branch has no required status checks: the branch protection (`protected` and
     `protection.required_status_checks` of `GET repos/{r}/branches/{base}`, readable with read
     access) and the rulesets (`GET repos/{r}/rules/branches/{base}`: no rule of type
     `required_status_checks`, `workflows`, `required_deployments` or `code_scanning`). GitHub
     waits for a required check forever, so a PR without a run cannot merge. GitHub's 403
     "Upgrade to GitHub Pro" on the rulesets read means that the plan has no rulesets: no
     ruleset applies. Every other error refuses.
3. **The evidence.** Then `done` records `ci: none (all files paths-ignored: <patterns>)`. The
   patterns are the ones that ignored the files, in file order, at most 8 (then `, and N
   more`). An allow-list that does not select a file shows as `outside paths of <workflow>`.
4. **Otherwise** the refusal is the old `evidence-ci-missing`, with the reason after "Not
   accepted as paths-ignored:", for example `file src/lib.rs is not ignored by workflow ci.yml
   (push does not ignore it)`, `workflow lint.yml cannot be read (its on: form is not known)`, `branch main requires the status checks ci / test; GitHub waits for them, so a PR
   without a run cannot merge`.
5. **`verify` and `merge`.** `verify` makes the same judgement for the head of the evidence when
   no run exists. A pass writes the fresh line into the `ci:` line of its result; a fail names
   the reason. The interactive `merge` makes it again, as it runs the `done` checks again
   ([`merge`](tickets.md#merge)). Such a ticket is not CI only for the headless merge and `tick`
   ([Headless coordinator](agents.md#headless-coordinator)).
6. **Read cost.** Only in the no-run case, and each read only if the step before passed:
   the workflow directory at the head SHA and each workflow file (1 + W, W at most 40; more
   than 40 workflow files refuse), then the base branch and its rulesets (2). `done` reads the
   PR files already (the owner route); `verify` and `merge` read them (up to 4 pages, enough to
   see "more than 300"). An unreadable CI source or more than 300 files refuse before any read.
7. **Glob semantics.** As GitHub's filter pattern cheat sheet: `*` any characters except `/`,
   `**` any characters (`**/` also zero directories), `?` zero or one of the character before,
   `+` one or more of it, `[...]` one character of the class, `\` makes the next character plain,
   and a leading `!` takes a match back. The list runs in order and the last pattern that
   matches decides. In `paths-ignore` a `!` pattern therefore takes a file back, and the file
   then starts the workflow.
8. **YAML forms that the reader supports** (the repo has no YAML crate; the hand-written reader
   of [CI must run for crew branches](#ci-must-run-for-crew-branches)): `on: push`,
   `on: [push, pull_request]`, a block list of events, a flow map
   `on: {push: {paths-ignore: [...]}}`, the block form with `push:`, `pull_request:` and
   `pull_request_target:` keys, and for each of their filter keys (`branches`,
   `branches-ignore`, `paths`, `paths-ignore`, `tags`, `tags-ignore`) a scalar, a one-line flow
   list or a block list (`- item` lines). A filter value in any other form (a flow list over
   several lines, a map, a nested list) makes the file unknown; so does a line in the `on:`
   block that is neither an event key nor a list item. Unknown is a refusal here and "cannot
   confirm" in the `init` warning, never a sure verdict.
9. **Limits.** GitHub compares a `push` with the commit before it, not with the base branch.
   crewboard judges the files of the whole PR, which is stricter in the usual case (a code
   commit before a docs commit refuses). A branch whose last push reverts a code change while
   the PR shows only docs could start a run that the judgement does not expect; that run then
   shows up and is judged as CI.

## Which CI runs count


An example: a Gate needed two runs on one head. The normal push run was green. A by-hand
`workflow_dispatch` run proved a fallback and was red on timing tests by nature. `done` counted
both runs, refused with `evidence-ci-not-green`, and `verify` refused too. The coordinator
moved the ticket by hand.

1. **Default.** `done`, `verify` and `merge` (which runs the `done` checks again, [`merge`](tickets.md#merge))
   count only workflow runs of the head SHA whose event is `push`, `pull_request` or
   `pull_request_target`. The code counted every event before. `pull_request_target` stays
   counted because it is a PR event. Its `head_sha` is the base commit, so it rarely matches the
   PR head.
2. **Information runs.** A run of any other event (`schedule`, `workflow_dispatch`,
   `repository_dispatch`, `workflow_run`, ...) is information. It is never green and never red.
   The check runs (jobs) of such a run are information too (matched by the check suite id).
3. **`--run <id>`** (repeatable, on `done`, `verify` and `merge`) names a run as evidence. The
   value is a run id (a run of the PR repo) or a run URL `https://github.com/<o>/<r>/actions/runs/<id>`.
   The run must be in a repo of the initiative (`evidence-run-foreign-repo`, checked before any
   call to that repo) and must belong to the head SHA (`evidence-run-wrong-sha`). A run that does
   not exist or an unreadable value is `evidence-run-ref`. A named run counts: it must be
   completed with conclusion `success`.
4. **A red run is never hidden.** If any counted `push` or `pull_request` run is red, the verdict
   is red, whatever `--run` adds. `--run` only adds runs. It never removes one.
5. **Commit statuses and check runs** work as before ([Evidence](tickets.md#evidence)). They combine with the runs by
   AND: every counted run, every check run that is not information, and every commit status must
   be green. A red status is red whatever `--run` names. Statuses never need `--run`.
6. **No counted run.** If the head has only information runs and `--run` names none (and no
   check run or status exists), the result is the missing-CI refusal `evidence-ci-missing` (the cause
   text). The text lists the information runs and says to name the Gate run with `--run`.
   A ticket that declares `CI: none` still records `ci: none`.
7. **The evidence text.** A green `done` records `ci-runs: counted: 1 (push, success), 5
   (workflow_dispatch, named, success); information only: 2 (schedule, failure)` in the signed
   message, and the command prints the line. `verify` records the fresh judgement in its result.
   A refusal for a red run adds the information runs as the last reason.
8. **Cases.** The judgement is a pure function over the runs. These cases hold, in both
   directions: push run green passes; PR run green
   passes; red push or PR run is red; only a green dispatch run is `OnlyInformation` (listed);
   red schedule run next to a green push run passes; red push run plus a named green dispatch run
   is red; a named run on another SHA is refused; a named run in a foreign repo is refused; a
   named green dispatch run alone passes and is named; a named red run is red; the red job of an
   information run is ignored and the red job of a push run is not; a red commit status stays
   red; the fetch reads and checks the named run. `done`, `verify` and `merge` apply the same
   facts, and all three pass the named runs to the CI fetch (`verify` and `merge` through one
   helper, point 10).
9. **Skipped and neutral check runs.** When no required-contexts list applies, a
   check run that is `completed` with conclusion `skipped` or `neutral` is ignored if at least
   one check run succeeded. If any other check run failed (`failure`, `cancelled`, `timed_out`,
   `action_required`, `stale`) or is still pending, the verdict is red as before. A SHA whose
   check runs are only `skipped` or `neutral` stays red: nothing ran. The ignored runs are named
   in the evidence (`ci-runs: ignored: desktop-app (skipped)`, after `counted` and
   `information only`), so the decision stays visible. With a required-contexts list nothing
   changes: every required check must have run and succeeded, so a skipped required check is
   still not green. That is stricter than GitHub, which treats a skipped required check as
   passing.
10. **The named runs of the evidence.** A run that `done --run` named counts again
    in `verify` and `merge` (and so in `tick`). The coordinator does not repeat `--run`.
    - **Source.** The run ids in the `counted:` part of the `ci-runs:` line of the done message
      that is judged, each with `(<event>, named, <outcome>)`. Example: `counted: 5
      (workflow_dispatch, named, success); ignored: desktop-app (skipped)` gives run 5. An
      entry is a run id (a run of the PR repo) or a run URL. `done` writes the URL for a named
      run of another repo of the initiative. An entry that does not parse, a URL of a foreign
      repo, and entries in `information only:` and `ignored:` never count.
    - **Same judgement as `done`.** Each run is read again. It must exist and belong to the
      evidence SHA, and it must be green now. A red run (for example after a rerun) is red. A
      run that is gone or ran on another SHA does not count: `verify` reports a failure with
      its rule (`evidence-run-ref`, `evidence-run-wrong-sha`), and `merge` refuses with it.
    - **`--run` only adds runs.** The runs of `--run` on `verify` or `merge` join the runs of
      the evidence. A run in both lists is read once. A bad `--run` value still refuses the
      command, as in point 3.
    - **No named run.** Evidence without a named run and without a counted run fails as in
      point 6.
    - **Headless.** A run by hand is not CI only. The headless `verify` counts the run, but the
      headless `merge` refuses evidence with a named run (`headless-scope`) before any read,
      and `tick` leaves the ticket for the interactive coordinator
      ([Headless coordinator](agents.md#headless-coordinator)).

## Jobs that ci_ignore skips

A job can be red for a reason outside the ticket, for example a job that fails on a spending
limit. The team lists such jobs in `AUTHORITY.md`; crewboard then does not count them.

```yaml
ci_ignore:
  - deploy-preview     # exact job name
  - "lint (*"          # prefix: ends with *
```

1. **Source.** `ci_ignore` is read from `AUTHORITY.md` on the default branch of the main repo,
   fresh, as all keys. It is never read from the PR branch: a PR cannot ignore its own red job.
2. **Match.** An entry matches a check run name or a commit status context exactly, or as a
   prefix when it ends with one `*`. Nothing else: no regex, no `*` inside the name. A lone `*`
   or a `*` inside the name is refused at parse (`authority-bad-front-matter`).
3. **Nothing by default.** A job that is not listed stays counted, also when it is red for
   infra reasons. Without `ci_ignore` the judgement is as before.
4. **A workflow run with a listed job.** The conclusion of a workflow run holds all its jobs. So
   a run that has a listed job is judged by its other jobs. The other jobs still must be green.
5. **Named, not silent.** The `ci-runs:` text names each ignored job, after the other parts:
   `ignored (AUTHORITY.md ci_ignore): deploy-preview = failure`. `done` prints it and records it,
   `verify` and `merge` judge again with the current list.
6. **One green job.** After the listed jobs are removed, at least one counted job must be
   left, and the judgement is not green if none is: the refusal is `evidence-ci-not-green`
   ("every CI job is ignored by AUTHORITY.md ci_ignore").
7. **Where.** `done`, `verify` and `merge` (so `tick` too). The read-only views (for
   example `mine` and the stale-work check) do not read the list and show the CI as before.

## Local run (ci_mode: local)

The CI of an initiative can stop working, for example on a spending limit or with broken
runners. The owner can then let the agents run the checks of the repo on a host and still deliver,
verify and merge through crewboard, with a record that the owner can audit. This is a decision of
the owner, set in `AUTHORITY.md` and switched back by the owner.

```yaml
ci_mode: local     # ci (default) or local
```

1. **Source.** `ci_mode` is read from `AUTHORITY.md` on the default branch of the main repo,
   fresh, as all keys. It is never read from the PR branch: a PR cannot switch it. Any other
   value than `ci` or `local` is refused at parse (`authority-bad-front-matter`). Without the
   key the mode is `ci`, and nothing in this section applies.
2. **`done` in `ci_mode: local`.** `crewboard done <n> --pr <url> --sha <head> --local-run
   "<host>: <command> rc=0, log <path>"`. `--local-run` is required (`local-gate-missing`
   without it, or with a blank text). The text is one line of at most 400 bytes
   (`local-gate-invalid` otherwise; the refusal names the actual length and the limit) and passes the check of every note (`secret-in-body`). The
   checks of the PR, the head and the branch are those of `done`. The CI of the head is NOT
   judged and not read: a red, pending or missing CI does not refuse. A check that runs anyway
   on a crew head (for example code scanning) is no proof of the tests, so `done` names no CI
   state. `poll` and `watch` give no `ci-finished` and no `green-not-done` in this mode
   ([Events](events.md#stale-work)).
3. **What is recorded.** The signed `done` message has `ci: local-gate`, the line
   `local-gate: <text>` and the line `CI off by owner decision (AUTHORITY.md ci_mode: local)`.
   The Evidence field has `ci=local-gate local-gate=<text>`.
4. **`verify` in `ci_mode: local`.** `crewboard verify <n> --local-run "<what the coordinator
   checked: host, command, rc, log>"`. It is required when the latest `done` is a local-run
   `done` (`local-gate-missing`). The head check stays: `evidence-head-moved` if the PR head is
   not the SHA of the `done`. The CI is not read. The signed verify report has the dev's
   `local-gate:` line and the coordinator's `coordinator-local-gate:` line.
   `verify --fail --body` needs no `--local-run`. A `done` with CI evidence is verified by CI as
   before, and `--local-run` is then refused (`local-gate-unexpected`).
5. **`merge` in `ci_mode: local`.** The CI part passes only if (a) `ci_mode` is still `local`
   at merge time, (b) the latest `done` is a local-run `done` with its line, and (c) its PASSED
   verify has a `coordinator-local-gate` line, with `sha:` and `head_now:` equal to the SHA of
   the `done`. Otherwise `local-gate-not-enabled` (a) or `local-gate-missing` (b, c). Every other
   check of `merge` stays, and the merge call carries the SHA. The notice to the dev names both
   local-run lines.
6. **`ci_mode: ci` (default).** `--local-run` on `done` or `verify` is refused with
   `local-gate-not-enabled` before any write. A local-run `done` from an earlier time does not
   count any more: `verify` fails it (`local-gate-not-enabled` in the failure text, so the ticket
   goes back to In Progress) and `merge` refuses it. The dev runs `done` again with CI.
7. **Headless coordinator.** `verify --local-run` is refused with `headless-scope`. `tick`
   leaves a local-run ticket ("left for the interactive coordinator"), and a headless `merge`
   of one is refused with `headless-scope`: a local run is a judgment
   ([Headless coordinator](agents.md#headless-coordinator)).
