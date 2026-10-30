# Contributing

This page tells you how to change crewboard itself. If you only run an initiative, read
[docs/guides/owner.md](docs/guides/owner.md) instead. For the map of the code, read
[docs/design/architecture.md](docs/design/architecture.md). For the behaviour, read the specification in [docs/design/spec/](docs/design/spec/README.md).

## Build

You need Rust 1.89 or newer (`rust-version` in `Cargo.toml`; edition 2024) and a C compiler:
the TLS library (`aws-lc-sys`) builds C code. On some targets it also needs `cmake` or NASM.
Rust is the language of crewboard itself, to build and test it. The projects that use crewboard
can be written in any stack; crewboard runs whatever their `.crewboard.toml` and tickets say.

The gate: run these before you push. CI runs the same commands.

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
```

CI also runs clippy on Windows. Code that is `#[cfg(unix)]` on one side only can pass on a Mac
and fail there. Type-check the Windows target on a Mac or Linux host before you push such a
change:

```sh
rustup target add x86_64-pc-windows-msvc
cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings
```

This checks types, names and lints. It does not link or run anything. The C build of
`aws-lc-sys` needs the Windows headers, which a Mac does not have. If the build script fails with
`windows.h file not found`, point it at stand-ins that only make empty files, because a type check
needs no object code. Set `CC_x86_64_pc_windows_msvc` to a shell script that writes an empty file
for its `-o` argument and exits 0. Set `AR_x86_64_pc_windows_msvc` to one that writes an empty file
for its `-out:` argument. Or install a real MinGW toolchain and use `x86_64-pc-windows-gnu`.
The `dead_code` lint is off on non-Unix hosts (`src/lib.rs`), because the wake and lock engines have
no command there.

The shell suites (see "Test tiers", item 3) need a built binary. Run them when you change
anything under `skill/`, `install.sh` or `scripts/`:

```sh
cargo build --locked
export CREWBOARD_REAL="$PWD/target/debug/crewboard"
bash skill/loops/tests/test-loops.sh
CB_IMPL=rust bash skill/loops/tests/test-loops.sh
bash skill/loops/tests/test-locks.sh
bash scripts/tests/test-install-release.sh
scripts/shellcheck.sh
```

The code has three lint rules that clippy enforces (`Cargo.toml`, `clippy.toml`): no `unwrap` or
`expect` outside tests, no `let _ =` that drops a `Result`, and no function over 150 lines.
`tests/layering.rs` keeps the module layers apart.

Tests that need no real GitHub use a fake GitHub on 127.0.0.1 (`tests/support/mod.rs`). The CLI
reads `CREWBOARD_API_BASE` and accepts only an `http://` loopback host.

Two tests prove interop with OpenSSH: crewboard signs and `ssh-keygen -Y verify -n crewboard`
accepts, and `ssh-keygen -Y sign` signs and crewboard accepts. If `ssh-keygen` is not in `PATH`
they print `SKIPPED` and pass. Set `CREWBOARD_REQUIRE_SSH_KEYGEN=1` to make a missing `ssh-keygen`
a failure (CI does this on Linux).

## Test tiers

Use the lowest tier that can prove your change.

1. **Offline tests.** `cargo test --locked` needs no network and no token. It runs:
   - the unit tests in `src/`;
   - the tests with a fake GitHub on a loopback port (`tests/component/fake_*.rs`, one module each
     of the binary `tests/component.rs`, for example `tests/component/fake_board.rs`,
     `tests/component/fake_tickets.rs`, `tests/component/fake_queue.rs` and `tests/component/fake_init.rs`;
     helpers in `tests/support/`; run one with `cargo test --test component fake_tickets::`).;
   - the guard `tests/md_links.rs`: every relative link in a tracked Markdown file must resolve;
   - the guard `tests/cli_reference.rs`: `docs/reference/cli.md` must match `--help`. After you
     change a command or an option, run `CREWBOARD_UPDATE_DOCS=1 cargo test --test cli_reference`
     and commit the page;
   - the guard `tests/neutral_names.rs`: no tracked file may name a project, agent,
     host or account (the list is a private maintainer file, see "Project-neutral rule");
   - the guard `tests/history_refs.rs`: no `issue #N`, `PR #N` or `(#N)` in `src/`, `tests/`,
     `skill/` or `docs/`. A comment says the rule and the failure it prevents, not a tracker number.
     A Rust comment says what the code does now, not its history, and a script `<name>.sh` that a
     comment or a doc names must exist in the repository;
   - the guard `tests/skill_eval.rs`: each situation of the agent skill must name its action in the
     same rule. A change to `skill/SKILL.md` or `skill/references/` must keep it passing. To add a
     case, append a `[[case]]` to `tests/fixtures/skill-eval/cases.toml` with a `role`, a
     `situation` and an `action`, copied from the skill text. Both texts must stand in one list
     item or one paragraph. If you reword a rule on purpose, change the case in the same commit;
   - the guard `tests/third_party_licenses.rs`: `THIRD-PARTY-LICENSES.md` lists every crate of
     `Cargo.lock` (see "Third-party licenses").
2. **Sandbox integration tests (optional, for maintainers).** `tests/it_sandbox.rs` and
   `tests/it_user_board.rs` talk to a real sandbox project that you provide. CI does not run them.
   They are `#[ignore]`d and run only with `CREWBOARD_IT=1` and
   `-- --ignored`. Without them plain `cargo test` lists them as ignored. `tests/it_sandbox.rs` also
   needs four variables, and skips with a message when one is unset or bad:
   `CREWBOARD_IT_OWNER` (login of the org or user that owns the project), `CREWBOARD_IT_PROJECT`
   (the project number), `CREWBOARD_IT_REPO` and `CREWBOARD_IT_REPO2` (two repos, each `owner/name`;
   repo 1 holds the registry issue). `tests/it_user_board.rs` needs `CREWBOARD_IT_USER=1` and
   `CREWBOARD_IT_CREW_LOGIN`. Recorded fixtures and unit tests use the placeholder `example-org`. Rules:
   - Run one run at a time per sandbox project, because one run retires the agents of another run.
   - Keep a reserve of REST quota. All `gh` calls of one owner share 5000 requests per hour, and one
     test can use more than 1500. Start a run only when `gh api rate_limit --jq .resources.core.remaining`
     shows at least 2000.
   - Run one test filter at a time, with `-- --test-threads=1`:

     ```sh
     CREWBOARD_IT=1 CREWBOARD_IT_OWNER=<owner> CREWBOARD_IT_PROJECT=<n> \
       CREWBOARD_IT_REPO=<owner>/<repo> CREWBOARD_IT_REPO2=<owner>/<repo2> \
       cargo test --test it_sandbox -- --ignored --test-threads=1
     ```

   The sandbox board is shared by every run, and each run makes new agent names
   (`<runtime>@it-<suffix>` and the `@itx-` agents of the lifecycle tests). Each name gets an option
   of the `Agent` field, and GitHub allows 100 options per field
   ([Option limit](docs/design/spec/board.md#option-limit)). The sandbox stays under the limit in two ways:
   - A run retires its agents when its process ends: a watcher process posts `/crew reject` and sets
     `State = left` for the three standing names and for every agent that a test made (they are
     noted in the run record under `~/.cache/crewboard-it/runs/`). `setup()` also retires the
     agents of earlier runs that provably finished.
   - `approve` in the setup prunes the options of retired agents that no item uses, oldest first,
     when the field is full. If `approve` still refuses with `agent-field-full`, run
     `crewboard status --initiative sandbox` as the owner, retire the agents it lists as running,
     or clear the `Agent` field of the leftover items.
3. **Shell tests.** `skill/loops/tests/test-loops.sh` and `skill/loops/tests/test-locks.sh` test
   `crewboard monitor`, `wake`, `lock` and the agent commands against a fake board. They need a
   built `crewboard` (on the `PATH`, or `CREWBOARD_REAL=target/debug/crewboard`). The drivers in
   `skill/loops/tests/drivers/` are thin wrappers that the suites use to run `crewboard wake`,
   `lock` and `repeat`; they are not part of the skill. The skill has no script. `crewboard skill
   install` is tested by `tests/skill_install.rs`.
   `scripts/tests/test-install-release.sh` tests the release installer with fakes.
   They run on macOS bash 3.2, so use no `declare -A` and no `mapfile`. `test-loops.sh` proves that a
   quiet board prints nothing and starts no model.

Every rule needs two tests: one that passes and one that is refused. A check that can never fire is
worse than no check.

## Pull requests

1. Make one branch and one PR per change.
2. Write "Closes #n" in the PR body when the PR fixes an issue. A PR for a step of a larger issue
   says "step k of #n" and does not close the issue.
3. Run the gate (see "Build") before you push.
4. Wait for CI. The workflow `.github/workflows/ci.yml` runs format, clippy, the tests and the
   docs build on Ubuntu and the same tests on Windows, and the shell suites on Ubuntu. These checks
   must be green. macOS is covered by `.github/workflows/build.yml`, which builds the release
   binaries, and by a local run of the gate on a Mac.
5. A maintainer reviews the PR. The maintainer merges with the full head SHA of the reviewed
   commit, so a head that moved is refused:
   `gh pr merge <n> --merge --match-head-commit <full sha>`.
6. A maintainer does not merge a PR that only the author has seen.

## Scope

crewboard is small on purpose. These changes are welcome without a prior discussion:

- bug fixes of existing behaviour;
- tests;
- docs, including a simpler `skill/SKILL.md`;
- measurement;
- release tooling;
- generality for other teams and stacks.

A new command, flag, rule or status transition changes the contract that agents rely on. Open an
issue first and describe the failure it prevents and the workaround with the current commands.

## Release process

A release is a tag `vX.Y.Z` on a commit of `main`. The version in `Cargo.toml` must be `X.Y.Z`;
`tests/skill_version.rs` keeps the skill version equal to it.

1. Update `CHANGELOG.md`, set the version in `Cargo.toml`, run `scripts/third-party-licenses.sh`
   and merge the change.
2. Push the tag. `.github/workflows/release.yml` calls `build.yml` for the tag. The build makes
   `crewboard-<version>-<target>.tar.gz` for each target (the binary, `README.md`, `LICENSE`,
   `NOTICE` and `THIRD-PARTY-LICENSES.md`) and a `SHA256SUMS` file, and the release job creates the
   GitHub release from them.
3. If the macOS job failed, no release is created: the checksum and release jobs need all four
   tarballs. Fix the cause and rerun the failed jobs of the run (`gh run rerun <run id> --failed`).
   `scripts/maintainer/release-macos.sh vX.Y.Z` is only for a draft release that already holds the
   Linux tarballs and `SHA256SUMS` but no macOS assets: it builds both macOS targets on a Mac,
   adds them and publishes the draft.

Install and upgrade paths are in [docs/guides/install.md](docs/guides/install.md) and
[docs/guides/upgrade.md](docs/guides/upgrade.md). The shell script `install.sh` and
`scripts/install-release.sh` install from the release assets.

## Third-party licenses

The binary contains the Rust crates of `Cargo.lock` and the C code of `aws-lc-sys`. Their licenses
require the license text and copyright notices with the binary. `THIRD-PARTY-LICENSES.md` holds
them, and every release archive ships it. After a change of `Cargo.lock`, run
`scripts/third-party-licenses.sh` and commit the file. It needs `cargo-about`
(`cargo install --locked --version 0.9.2 --features cli cargo-about`); the settings are in
`about.toml` and `about.hbs`. `scripts/third-party-licenses.sh --check` tells whether the file is
current, and `tests/third_party_licenses.rs` fails when a crate is missing. A new dependency
needs a license that is on the `accepted` list of `about.toml`: Apache-2.0, MIT, BSD, ISC and similar
permissive licenses. Ask before you add a copyleft crate.

## Dependabot

Dependabot opens weekly pull requests for Cargo crates and GitHub Actions (`.github/dependabot.yml`).
Dependabot cannot run `cargo-about`, so a Cargo update fails `tests/third_party_licenses.rs`
until the license file is current. Fix it like this:

1. Check out the PR branch and merge `origin/main` into it.
2. Run `scripts/third-party-licenses.sh`.
3. Commit `THIRD-PARTY-LICENSES.md` to the PR branch and push.

For an Actions update, the PR's own CI must be green. Read the release notes of every major
version between the old and the new one, and look for breaking changes in the inputs and
outputs that `.github/workflows/` uses. Merge only after that check.

## Writing style

- Write documents and ticket text in ASD-STE100 (Simplified Technical English): one idea per sentence,
  short sentences, active voice, the same word for the same thing.
- Do not use em-dashes. Use a hyphen or a new sentence.
- Keep domain terms as they are. Do not force the approved word list where it harms the meaning.

## Project-neutral rule

crewboard names no specific project, agent setup, host or account. Use placeholders in examples:
`<owner>/<repo>`, `<initiative>.env`, `claude@host-a`, `hermes@host-b`, `codex@host-c`. Rotate the
runtimes in examples, and write `--model <model>`: a model name reads as a requirement.
Name no person and no personal tool; the git log credits authors. Say "the sandbox project that the
`CREWBOARD_IT_*` variables name", not its owner or repo names. Fixtures and unit tests use
`example-org`.
The test `tests/neutral_names.rs` keeps private names out of the tree. A failure prints `file:line`
and the hit. The comments in the test say how to extend it.
Maintainers keep a private name list for this test, outside the tree (the path in
`CREWBOARD_NEUTRAL_NAMES_FILE`, default `~/.config/crewboard-dev/neutral-names.txt`). Without that
file the list check prints `neutral_names: no list at <path>; skipped` and passes, so your run is
green. The word check and the structure checks need no list and always run.

## Host lock for heavy jobs

On a host that several agents share, a heavy job (a build, a full test run) takes the host lock first,
so it does not starve the others. See [docs/guides/ci-and-host-lock.md](docs/guides/ci-and-host-lock.md).
The command is `crewboard lock` (`src/lock/`); It has no shell wrapper.
