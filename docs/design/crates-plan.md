# Crates of the workspace

This page records how the code is cut into crates, and the question that is still open.

## Decision

The repo is a Cargo workspace with three crates:

- `crewboard`, the root package: the binary and its library (`src/`).
- `crewboard-protocol` (`crates/crewboard-protocol`): the wire of protocol v1 (header, canonical
  form, signature, replay window, former names, message acts).
- `host-lock` (`crates/host-lock`): the host lock and the four host seams that it shares with the
  binary: `clock`, `spawn`, `fsutil` and `ignore`.

The root package stays at the repo root. `cargo install --path .`, the clone check of
`skill install` (`[package] name = "crewboard"`), `CARGO_BIN_EXE_crewboard` of the integration
tests and the skill embed of `build.rs` all need it there.

The two lower crates depend on no crate of the workspace. `tests/layering.rs` checks their
manifests, and Cargo stops them from importing the rest. `src/lib.rs` re-exports them under their
old module names, so `crate::protocol::...`, `crate::lock::...` and `crewboard::clock` stay valid.

The seams are in `host-lock` because the lock takes the clock of its caller
(`Session.clock: &dyn Clock`), so the trait must exist once, below both. A dependency of
`host-lock` on `crewboard-protocol` was rejected: it would bring `ssh-key` and `sha2` into the
lock for one 10-line calendar rule. `clock` keeps its own copy of that rule instead, and
`tests/calendar_copies.rs` keeps the two copies equal for every day from 1899 to 2101.

## Crate graph

```text
  crewboard (src/: main, cli, wake, ops, and every other module)
     |                         \
  crewboard-protocol           host-lock
  (sha2, ssh-key)              (libc; lock, clock, spawn, fsutil, ignore)
```

## Open question: a crate `crewboard-core`

A third library crate could take every module that the command layer (`cli`, `wake`, `ops`,
`ctx`, `unread`) imports: about 82 files and 39k lines. The binary would keep the command layer
and `skill_embed` (about 59k lines). No module outside the command layer imports one inside it,
so the cut has no cycle.

The question is open. These are the reasons against the split now:

- **Build time.** The measurements below show no build-time problem that the split could solve.
  Most edits are in `ops` (37k lines), and `ops` recompiles in both layouts. After an edit in
  `rules`, `config` or `github`, two crates would rebuild one after the other.
- **What it enforces.** `tests/layering.rs` already freezes the module edges between the command
  layer and the rest. The crate boundary adds nothing that `no_new_module_edges` does not refuse.
- **What it costs.** About 100 renamed files, which conflict with all work in flight. About 110
  `pub(crate)` modules and items become `pub`, and rustc does not report an unused `pub` item, so
  the dead-code check stops for the core API. The test helpers that both sides use (`testkit`,
  `registry::tests`, `comments::tests`) need a `test-support` feature.
- **Reversible.** The re-exports keep every path stable, so the split can follow later without
  rework.

## Build times (2026-11-01)

Debug builds on a 12-core Apple silicon Mac, load average 7 to 12 from other work, so the numbers
are approximate. One edit is one new item in one file.

| Build | Time |
|---|---|
| Clean `cargo build` (all dependencies) | 26.7 s |
| The `crewboard` library in that build | 9.8 s, from 16.2 s to 26.0 s |
| `crewboard-protocol` and `host-lock` in that build | 1.3 s and 1.8 s, in parallel with the dependencies |
| `cargo build` after an edit in `src/ops/say.rs` | 2.2 to 3.0 s |
| `cargo build` after an edit in `src/rules/mod.rs` | 2.1 to 2.2 s |
| `cargo build` after an edit in `crates/host-lock/src/judge.rs` | 2.5 s |
| `cargo test --workspace --no-run` after an edit in `src/ops/say.rs` | 5.1 to 5.9 s |
| `cargo test --workspace --no-run` after an edit in `src/rules/mod.rs` | 4.9 to 5.0 s |

The critical path of a clean build is the dependencies (`reqwest`, `aws-lc-sys`) and then the
`crewboard` library. An edit costs 2 to 6 seconds wherever it is. A core crate could save only a
part of these seconds after an edit in the command layer.
