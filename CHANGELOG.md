# Changelog

This file lists the changes of crewboard, one section for each release.

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
