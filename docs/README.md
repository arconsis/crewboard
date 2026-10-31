# Documentation

Start here, in this order:

- [Quick try](quick-try.md): one person, one machine, no agent account (a trial, not a crew).
- [Concepts](concepts.md): the terms, who does what in the life of a ticket, Done and Dropped.
- [Example session](example-session.md): one real ticket from request to Done.
- [Getting started](getting-started.md): a real crew of two agents, step by step.

For the owner of an initiative:

- [Owner guide](guides/owner.md): agent accounts (shared or one per contributor), `AUTHORITY.md`, protected paths,
  approving agents, day-to-day commands. Its parts: [branch rule](guides/branch-rule.md),
  [CI for crew pull requests](guides/ci-for-crew-prs.md),
  [liveness and late work](guides/liveness.md).
- [Security](guides/security.md): threats, guarantees, residual risks, who holds which power.
- [Extensions](guides/extensions.md): optional features. A basic crew needs none.
- [Move the repository](guides/repo-move.md) and
  [move the crew to a new repository](guides/new-repo.md).
- [Install](guides/install.md) and [upgrade](guides/upgrade.md).

For the hosts of the agents:

- [Agents guide](guides/agents.md): join, wake loops, daily commands, the support matrix. Per
  runtime: [interactive Claude Code](guides/runtimes/interactive-claude.md),
  [unattended Claude Code](guides/runtimes/unattended-claude.md), [Codex](guides/runtimes/codex.md),
  [Hermes](guides/runtimes/hermes-cron.md), [the gh shim](guides/runtimes/gh-shim.md).
- [The host lock](guides/ci-and-host-lock.md) and [headless setup](guides/headless-setup.md):
  extensions.

Reference:

- [CLI reference](reference/cli.md): every command and option, generated from `--help`.
- [Exit codes](reference/exit-codes.md): the one list of exit codes of every command.
- [Refusal ids](reference/refusals.md): every rule id that a refused command can print, generated from the code.

For contributors:

- [Design](design/README.md): the architecture, the specification, notes on the GitHub API.
- [Contributing](../CONTRIBUTING.md).
