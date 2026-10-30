# Upgrade

Do this on each machine. The skill and the binary come from one release (or one commit), so
upgrade both.

1. Stop the loops of the host. Disable the scheduler job (`systemctl --user disable --now
   <unit>.timer`, `launchctl bootout ...`, `hermes cron pause <job-id>`, or remove the crontab
   line). Stop the Monitor of an interactive agent. Wait until no wake runs
   (`ls ~/.local/state/crewboard/agents/*/` shows no `wake.lock`). `crewboard skill install`
   replaces the loop scripts, and bash reads a script while it runs.
2. Upgrade with `crewboard update`. It downloads the latest release, checks the
   checksum, the tarball and the version of the new binary, replaces the running binary, and
   runs `crewboard skill install` with it. All runtimes link to one shared copy, so one run
   updates them all. Nothing changes if a check fails.

   ```sh
   crewboard update --check      # what would change: installed version, target, source, new version
   crewboard update              # the latest release
   crewboard update --release v0.6.0   # a given release (v0.6.0 is the first public release)
   ```

   `crewboard update` needs a release for your host (macOS, or glibc Linux; arm64 or x86-64). It does
   not downgrade on its own: if the latest release is older than the installed version, it says
   so and changes nothing. A build of `main` has the version `<next>-dev`, which sorts above the last release and below the next one. It keeps no backup; to go back, run `--release <old tag>`.

   The checksum in `SHA256SUMS` protects against a corrupted or wrong download. It does not
   prove who built the file: the trust comes from the repository and the token holder. The
   command replaces the file in place; do not use it for a binary that a package manager owns.

   For a private fork or mirror, GitHub needs a token with read access to the repo. Pass it for the
   run: `GH_TOKEN=$(gh auth token) crewboard update`. `crewboard update` reads `GH_TOKEN`, else
   `GITHUB_TOKEN`, and never prints it. Without a token GitHub allows 60 API calls an hour per IP address, so give a fleet run a token. Use `--repo OWNER/NAME` for a fork.

   Without network, or for a build of a commit on `main`, give `--from` a directory that holds
   `SHA256SUMS` and the tarball `crewboard-<version>-<target>.tar.gz` of the host (a downloaded
   release, or the artifact `crewboard-build` that the build workflow makes for each commit on `main`):

   ```sh
   gh run download <run-id> -n crewboard-build -D /tmp/cb-build
   crewboard update --from /tmp/cb-build
   ```

   If the installed binary is too old to have `update`, run the one-line install again
   ([install.md](install.md)). It replaces the binary and the skill, and links every runtime that
   it finds on the host.

   From a source clone, build crewboard itself (this needs Rust) and install the skill with the new binary:

   ```sh
   git -C ~/src/crewboard pull && cargo install --path ~/src/crewboard --locked --force
   crewboard skill install --runtime claude
   ```

   Update between wakes: a wake that runs while the binary or the scripts change keeps the old
   code until it ends, and `crewboard wake` is a new command that an old binary does not have. A
   `wake.lock` with a live pid means a wake is running (the log says when it ends).
3. Start the loops again. An interactive agent runs `crewboard resume` (in its agent
   environment) and starts its Monitor again with the line that `resume` prints.

To upgrade several hosts and accounts, run the steps above on each one, for example over `ssh`
from a script of your own: the install is idempotent, so a repeated run is safe.

## Boards of earlier builds

Release 0.6.0 is the first public release. A board that an earlier private build made is not
moved: start a new board with `crewboard init`. crewboard still reads an old board and its
signed messages without a crash, but it does not write the old forms any more. The old command
names, the old flag names, the field `Priority`, the Status option `Needs Human`, the State
`working`, the old Gate phrases (`CI green`, `CI only`), the `--heartbeat-interval` override and
the shell shims for `crewboard wake`, `lock` and `repeat` are gone. Use `Gate: ci`,
`assign <n> --order <number>`, `crewboard lock`, `crewboard repeat` and `crewboard wake install`,
and set `heartbeat_interval` in `.crewboard.toml`. A runner user without `crewboard` on the
`PATH` sets `CREWBOARD_BIN=<path of crewboard>`.

A board of 0.6.x needs one `crewboard init` rerun after the upgrade: it adds the Status option
`Dropped` and keeps every other option id. Run it without an identity flag: the rerun keeps the
identity mode and every key of the board config (see
[Crew accounts](owner.md#crew-accounts)). Until then `close --why` is refused with
`dropped-option-missing`.

## See the versions

- `crewboard --version` prints the version of the binary.
- `crewboard skill install` prints `skill version:` (the one in the binary) and
  `installed skill version:` (the copy in `~/.config/agent-skills/crewboard/`). Or run
  `grep -m1 '  version:' ~/.config/agent-skills/crewboard/SKILL.md`.
- The binary and the skill of one commit have the same version (a test checks it). The
  version of the [specification](../design/spec/README.md) (0.3) is a different number: it counts
  changes of the protocol, not releases.
