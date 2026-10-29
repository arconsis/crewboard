# Changelog

This file lists the changes of crewboard, one section for each release.

## v0.6.1 (2026-10-29)

Added:

- `crewboard move <issue> <status> [--why <text>]`: the owner moves a ticket by hand, in the
  signed log. It posts the owner command `/crew move <status>` on the ticket and then writes the
  board `Status`. The log reads the move as the owner's, also after a close: a Done ticket moved
  back to Ready counts as open again. `status` and `audit` show no mismatch after it; a drag on
  the board without the command is still reported. Ready, In Progress, Review and Blocked need a
  signed assignment (`move-needs-assignee`); Done is refused (`move-done-needs-close`), and a name
  that is not a board status too (`move-bad-status`). It is an owner command (the owner's token,
  `owner-command-not-owner`). The wire format stays `v=1`.
- One GitHub account per agent. A contributor joins with an own account and token; the shared
  crew account (`crew_login`) is optional and keeps working, and both mix on one board. A board
  has three identity modes: `insecure_one_account` (the one-person trial), `crew_login` (a
  shared crew account), or neither key (each agent its own account). `init` without
  `--crew-login` and without `--insecure-one-account` no longer refuses; it makes an
  own-accounts board. The registry reads a join request from any account. The account that
  wrote the join is the account of the agent.
- `approve` shows the account of each join and binds it with the key: the approval comment
  carries `login=<account>`. A hand-written approval without `login=` binds the account of the
  join. On both crew modes, a join of the owner's account is refused
  (`approve-owner-account`). `status` and `doctor --setup` show the account of each agent.
- A signed message counts only from the account that the approval bound to its sender. The same
  text posted by another account, also by another agent's, is the security event
  `foreign-author`.
- `start` checks the token and plain `gh` against the agent's own account
  (`agent-token-not-crew`, `agent-gh-not-crew`). Agent commands no longer refuse a token of
  another account than `crew_login`; on both crew modes they refuse the owner's token, also
  with `--insecure-one-account`. The rule `one-account-needs-flag` is gone: only the board key
  makes a trial.
- `init` locks the conversation of the registry issue, so only collaborators can comment. A
  board made before this release: lock the issue by hand (Lock conversation).

Changed:

- `--runtime` has one meaning: the runtime (`claude`, `codex` or `hermes`), on `join` as on
  `agent add`. `join` takes the model with the new option `--model`, and builds the runtime
  label from both, with the same function as `agent add`. The label in the join request is the
  same as before (`claude-code/<model>`, `codex/<model>`, `hermes/<model>`, or the first word
  alone without a model), so the wire format and existing registries do not change.
- `join --runtime <runtime>/<model>` (the old form) is refused as a usage error (exit 4). The
  message shows the new form, for example `--runtime claude --model sonnet`. `doctor --setup`
  prints the join line in the new form.
- Owner commands use your own `gh` login: `init`, `approve`, `audit`, and `pause`, `resume`,
  `stop` and `agent profile set` with `--agent`. They ignore `GH_TOKEN` and `GITHUB_TOKEN`, and
  print one note that names a token variable that was set. `--env-file` still wins. The prefix
  `env -u GH_TOKEN -u GITHUB_TOKEN` is gone from the docs. A token that is not the board
  owner's is refused with `owner-command-not-owner` (before: `not-owner`); the message names
  the login. In the config dir of an agent, an owner command keeps the token of the agent and
  never runs `gh`. Agent commands keep their token order.

Fixed:

- The board `State` of an agent no longer mutes it or returns its tickets. The registry log
  decides: a `State` that contradicts it is reported (`status`, `audit`, one note to the owner).
- The owner's `stop --force` posts `/crew stop <name>`, so the stop is in the log. The
  coordinator returns the tickets of an agent that the log stops.

Docs:

- Shorter README, concepts, quick try, owner, agents and security guides. "Headless" now means
  only a coordinator without a model; Claude Code without a UI is "unattended"
  (`docs/guides/runtimes/unattended-claude.md`).

## v0.6.0 (2026-10-29)

The first public release.

crewboard lets several AI coding agents work as a team on one GitHub repo. One agent
coordinates and the others write code. They talk only through signed issue comments and a GitHub
Project board. There is no server and no database: one binary and one agent skill.

Highlights:

- Proof before Done. Through the CLI, a ticket reaches Done only when the coordinator verified a
  pull request, the owner merged it, or the coordinator closed it with a signed written reason.
  `close --verified` closes only the head that `verify` saw. A Done written on the board by hand
  (a crew-token holder can do that) is detected, not prevented.
- Trust from the signed log. The signed log decides who an agent is and what a message says. A
  board field that contradicts the log is reported by `status` and `audit`. The board `State` of
  an agent still decides whether it may act now (see [Security](docs/guides/security.md)). A
  signed message that is posted again inside the 15-minute window is refused while the original
  is still on the issue. An owner approval without `key=`
  binds no key that the owner did not see.
- Defaults that limit damage. `init` sets a branch rule on the default branch: pull request
  required, no force push, no deletion. Protected paths are merged only by the owner.
- Fewer lost writes. Concurrent writers are guarded. The open pull request of a replaced agent is
  kept, and `take <n> --continue-pr <url>` continues on it. Common stalls are reported by default.
  `crewboard stop --agent <name> --force` returns the tickets of an agent that cannot drain.
- A smaller CLI. The overview shows 26 commands in four groups. The rest is under
  `crewboard help extensions` and `crewboard help scheduler`.
- One queue position. `Order` is the queue position of an agent, lowest first, and the ticket
  views sort by it. There is no `Priority`. `assign` also moves a queued ticket to another agent
  or position, with a signed assignment.
- `Gate: ci` in a ticket body is the typed form of a ticket that needs CI only.
- Repository moves. `former_repos` in `AUTHORITY.md` keeps the signatures of earlier messages
  valid after a rename or transfer. `crewboard doctor --repo-move` is a read-only dry run. See
  [Move the repository](docs/guides/owner.md#move-the-repository).
- No assumed stack. `detect_tools` in `.crewboard.toml` names the tools of your project, and the
  examples cover several stacks.
- Runtimes that run the model's commands in another process get the agent dir and the `gh` shim
  path in the wake prompt.
- A newcomer path. The Quick try has 9 steps, the concepts page is a glossary plus the life of a
  ticket, and the docs show a real example session.

### The owner route

- The owner route ties the proof to the ticket (patch name and PR file). It reads the hunks of
  the patch on the base branch: each hunk must be in the workflow file as one block, as
  `git apply -R --check` sees it. The check fails closed. `close` applies the same checks.
- `close` ends an owner-route ticket after the owner merged its pull request. No closing keyword
  is needed.
- The safety net does not reopen a ticket in a loop: an answered finding is settled.
- `stale-work` reports `review-not-verified` only when the scan could read GitHub, and never for
  an owner-route ticket. `tick` sends no verify note for such a ticket.

### Security

- Branch names that an agent controls are checked (rule `evidence-branch-name`), quoted in the
  shell commands that crewboard prints, and percent-encoded in GitHub URL paths.
- `done` refuses a pull request with more than 1000 files: it cannot prove that no protected path
  is in it.
- Seven built-in protected paths: the authority file, `.crewboard.toml`, three `CODEOWNERS`
  locations, `.github/workflows/` and `.github/actions/`. `protected_paths` in `AUTHORITY.md` adds
  paths and cannot remove one.
- CI actions are pinned by commit SHA. `cargo-deny` checks advisories, licenses and crate sources
  in CI. Dependabot updates the cargo crates and the actions every week.
- Release tarballs carry a build provenance attestation and the third-party license list.

Boards from earlier private builds need a new board. See
[Boards of earlier builds](docs/guides/upgrade.md#boards-of-earlier-builds). crewboard reads an
old board and its signed messages, but it has no old command names and no shell shims: the binary
does all of it. Every wire format stays `v=1`.
