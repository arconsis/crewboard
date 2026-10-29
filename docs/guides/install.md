# Install crewboard

## Install in one line

```sh
curl -fsSL https://raw.githubusercontent.com/arconsis/crewboard/main/install.sh | bash
crewboard --version
```

Use it on the owner machine and on each agent host. It downloads a prebuilt release, checks the
checksum, installs the binary to `~/.local/bin`, and runs `crewboard skill install`. The binary
and the skill come from one release, so they have the same version. Upgrade with
`crewboard update` (see [Upgrade](upgrade.md)).

Other ways: [the manual download](#manual-download) (the same steps by hand), and
[build from source](#build-from-source) (for contributors, and for a platform without a prebuilt
binary).

## What you need

- macOS (arm64 or x86_64) or Linux (x86_64 or arm64, glibc 2.35 or newer: for example Debian 12
  or Ubuntu 22.04). Windows has no prebuilt binary: [build from source](#build-from-source).
- `curl`, `bash`, `tar`, and `sha256sum` or `shasum`.
- Your user is enough. The script needs no root.

## Options

Pass options after `bash -s --`:

```sh
curl -fsSL https://raw.githubusercontent.com/arconsis/crewboard/main/install.sh | bash -s -- --version v0.6.0
```

| Option | Default | What it does |
|---|---|---|
| `--version vX.Y.Z` or `latest` | `latest` | The release to install. `0.6.0` and `v0.6.0` are the same. |
| `--prefix <dir>` | `~/.local` | The binary goes to `<dir>/bin/crewboard`. |
| `--runtime <name>` | every runtime that is present | `claude`, `codex`, `copilot` or `hermes`. Links the skill into that runtime even when it is not found. Use it more than once for more runtimes. |
| `--dry-run` | off | Downloads and verifies the release in a temporary directory and prints the plan. It changes nothing else. |

`<prefix>/bin` must be on your `PATH`. The script prints a note if it is not.

## Which runtimes get the skill

Without `--runtime`, `crewboard skill install` links the skill into each runtime that is present
on the host. A runtime is present when its skills directory or its config directory exists
(`~/.claude`, `~/.codex`, `~/.copilot`, `~/.hermes`), or when its binary (`claude`, `codex`,
`copilot`, `hermes`) is on `PATH`. The output names the runtimes that got a link and the ones
that were skipped. A host with no runtime, such as the owner machine, gets the shared copy in
`~/.config/agent-skills/crewboard/` and one line that says so. With `--runtime`, the named
runtime gets a link in any case.

## What the script does

In this order. If one step fails, it stops, and nothing is installed:

1. It finds the OS and the CPU, and the matching target (for example
   `aarch64-apple-darwin`). A platform without a prebuilt binary is refused with a message.
2. It finds the release tag (the latest, or `--version`) on the public release pages of GitHub.
3. It downloads two assets of the release with `curl`: the binary tarball and `SHA256SUMS`.
4. It verifies the sha256 of the tarball. A mismatch is refused.
5. It runs the downloaded binary once (`crewboard --version`) and checks the version.
6. It runs `crewboard skill install --dry-run` of the downloaded binary to check the options (for
   example the runtime names).
7. It installs the binary to `<prefix>/bin/crewboard`, then runs `crewboard skill install` with
   it. The skill is inside the binary; it lands in `~/.config/agent-skills/crewboard/`.
8. It prints `crewboard --version` and the next step.

Run it again to repair: it is idempotent. To upgrade, use `crewboard update`. Stop the loops of
the host first, as in [Upgrade](upgrade.md).

## Private fork or mirror

`curl` gets a 404 for a private repository. The script then uses `gh release download`. You
need `gh` 2.17 or newer, logged in as you (`gh auth login --hostname github.com`) with read access
to the repository. If you have no access, ask a maintainer. The crew account never needs
it. Unset `GH_TOKEN` and `GITHUB_TOKEN` in the shell, because `gh` uses such a token instead of
your login. Get the script with `gh` and run it:

```sh
gh api -H 'Accept: application/vnd.github.raw' repos/<owner>/<name>/contents/install.sh \
  | CREWBOARD_RELEASE_REPO=<owner>/<name> bash
```

`CREWBOARD_RELEASE_REPO` makes the script install the releases of that repository; without it,
the script installs from `arconsis/crewboard`. Options work as above:
`... | CREWBOARD_RELEASE_REPO=<owner>/<name> bash -s -- --version v0.6.0`.

## Manual download

Do the same steps by hand, for example to read each file first. Set the version and the target.
The targets are `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and
`aarch64-unknown-linux-gnu`.

```sh
V=0.6.0; TARGET=aarch64-apple-darwin
mkdir -p ~/crewboard-$V && cd ~/crewboard-$V
gh release download v$V --repo arconsis/crewboard \
  --pattern "crewboard-$V-$TARGET.tar.gz" --pattern SHA256SUMS
grep -E " crewboard-$V-$TARGET\.tar\.gz\$" SHA256SUMS | shasum -a 256 -c   # Linux: sha256sum -c
tar -xzf "crewboard-$V-$TARGET.tar.gz"
mkdir -p ~/.local/bin && cp "crewboard-$V-$TARGET/crewboard" ~/.local/bin/ && crewboard --version
crewboard skill install --runtime claude
```

Expect one `OK` line from the checksum check. Do not go on if the line says `FAILED`. The
`grep` keeps only the line of your file, so the check also works with an old `shasum`
that has no `--ignore-missing`.

## Verify a download

Two checks exist. They answer different questions.

The checksum shows that the file is not damaged. `install.sh` runs it for you. By hand, use the
`grep` and `shasum -a 256 -c` line of [Manual download](#manual-download) (Linux: `sha256sum -c`).
`SHA256SUMS` comes from the same release as the tarball. It does not show who built the file.

The build attestation shows that GitHub Actions built the file from this repository. Release
tarballs carry one from the first public release on. Check it with the GitHub CLI:

```sh
gh attestation verify crewboard-$V-$TARGET.tar.gz --repo arconsis/crewboard
```

`install.sh` does not need `gh` and does not run this check. Releases that a private fork or
mirror builds have no attestation.

## Build from source

This section builds crewboard itself, so it needs Rust. The projects that a crew works on need
nothing of the kind: crewboard does not depend on their stack. Contributors build from source. Do the same on a platform without a prebuilt binary. You need
Rust 1.89 or newer and a C compiler and linker: on macOS the Xcode Command Line Tools
(`xcode-select --install`), on Debian or Ubuntu `sudo apt install build-essential`. The first
build takes some minutes.

```sh
mkdir -p ~/src && gh repo clone arconsis/crewboard ~/src/crewboard && cd ~/src/crewboard
cargo install --path . --locked && crewboard --version   # to build crewboard itself; puts it in ~/.cargo/bin
crewboard skill install --runtime claude                 # prints the skill version
```

`~/.cargo/bin` must be on your `PATH`. For the development of the skill, `crewboard skill install --link`
(in the clone, or with `--from <clone>`) links the shared path to the clone instead of a copy. See
[Agents guide](agents.md) and [CONTRIBUTING.md](../../CONTRIBUTING.md).

## How a release is made

A maintainer pushes a tag `vX.Y.Z`. `.github/workflows/release.yml` builds the tarballs, checks
them against `SHA256SUMS`, attests the build provenance (public repository only, see
[Verify a download](#verify-a-download)) and creates the GitHub release. If the macOS job fails,
no release is created. The steps are in
[CONTRIBUTING.md](../../CONTRIBUTING.md#release-process).

### Builds of each commit

`.github/workflows/build.yml` builds every commit on `main` in release mode, and the release
workflow reuses it for a tag. Each build checks that `crewboard --version` prints the
`Cargo.toml` version and that the binary has the right CPU architecture, and packages
`crewboard-<version>-<target>.tar.gz` (the binary, `README.md`, `LICENSE`, `NOTICE` and
`THIRD-PARTY-LICENSES.md`; the skill files are inside the binary). A missing file of this list
stops the build. The run keeps one artifact, `crewboard-build`, for 30 days: the tarballs,
`SHA256SUMS` and `COMMIT` (the full sha of the built commit). A deployment downloads it instead
of building on each machine:

```sh
id=$(gh run list --repo arconsis/crewboard --workflow build.yml --branch main --status success \
  --limit 1 --json databaseId --jq '.[0].databaseId')
gh run download "$id" --repo arconsis/crewboard -n crewboard-build -D build
cd build && sha256sum -c SHA256SUMS && cat COMMIT   # macOS: shasum -a 256 -c SHA256SUMS
```

A change of `build.yml` or `release.yml` in a pull request also starts a build (it builds the head
commit of the PR). Use the artifact of such a run only to test the workflow.

#### macOS builds

The public repository builds both macOS tarballs on one GitHub-hosted Apple Silicon runner
(`macos-15`). The build of `x86_64-apple-darwin` is a cross build: CI checks it with `file` and
`lipo` and does not run it. A private fork or mirror builds on its own self-hosted macOS runner
instead, because the job chooses the runner by the visibility of the repository. The job never
runs for a pull request. A build of a push or a tag needs all four tarballs. If the macOS job
fails, no release is created: rerun the failed jobs (see above).

The job uses the environment `macos-build`. Restrict the deployment branches of that
environment to `main` and the tag pattern `v*` in the repository settings. A change of the
workflow file cannot remove this rule.

Set up a self-hosted runner with care, because the job builds code of the repo on that machine:

- Use a runner that only this repository can use (a runner group limited to the repository).
- Take the runner offline only with care. A job that waits for a runner waits up to 24 hours,
  and the `crewboard-build` artifact and each release wait with it.
