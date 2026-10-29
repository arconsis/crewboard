# Headless setup: a guardrail and a separate OS user

> **Optional hardening.** This page needs `sudo` and a second OS user. A crew of two agents
> does not need it: the [Getting started](../getting-started.md) path
> works without it. Use it when a no-model coordinator runs unattended and you want the
> process to have its own OS user and token.

Read this if a no-model coordinator runs without a person at the keyboard and you do not
trust the model with the full rights of the coordinator. The rules are in
[No-model coordinator](../design/spec/agents.md#headless-coordinator).

The headless scope is switched on by one key: `headless = true` in
`<config-dir>/<initiative>.toml`. The key is a guardrail, not a boundary. The headless OS user
can write its config dir, so it can remove the key or pass `--config-dir` to another dir. The
boundary is the OS user and its token: the steps below give the no-model coordinator its own
user, its own config dir and a limited token. `crewboard doctor` tells you whether the key is
on.

In the commands below, `cb-headless` is the headless OS user, `<initiative>` is the
initiative name and `<agent>` is the agent name of the coordinator (for example
`claude@host-a`). Run the `sudo` commands as an administrator.

## 1. Make the headless OS user

The user needs no admin rights.

macOS:

```
sudo sysadminctl -addUser cb-headless -fullName "crewboard headless" -shell /bin/zsh -home /Users/cb-headless -password -
sudo createhomedir -c -u cb-headless
```

`-password -` asks for the password. To make the user with `dscl` instead, pick a free uid
(`dscl . -list /Users UniqueID`), then:

```
sudo dscl . -create /Users/cb-headless
sudo dscl . -create /Users/cb-headless UserShell /bin/zsh
sudo dscl . -create /Users/cb-headless UniqueID 550
sudo dscl . -create /Users/cb-headless PrimaryGroupID 20
sudo dscl . -create /Users/cb-headless NFSHomeDirectory /Users/cb-headless
sudo createhomedir -c -u cb-headless
```

Linux:

```
sudo useradd --create-home --shell /bin/bash cb-headless
```

## 2. Install crewboard where the headless user cannot change it

The headless user must run a build that it cannot replace. Building crewboard from source needs
Rust (`cargo`); that is for crewboard itself, not for the project of the crew. A prebuilt release
binary needs no build.

```
cargo build --release --locked
sudo install -o root -m 755 target/release/crewboard /usr/local/bin/crewboard
```

On macOS add `-g wheel`, on Linux `-g root`. Put `/usr/local/bin` first on the `PATH` of the
headless wake. Do not put a `crewboard` in a directory that the headless user can write.

## 3. Give the headless user its own config dir

The config dir holds the agent config, the signing key and the state cache. It belongs to the
headless user, and only that user can read it. The layout is the same as for every agent
(`~/.config/crewboard/agents/<agent-dir>`, see [Agents guide](agents.md)). Here
`<agent-dir>` is the agent dir of the coordinator in your own home, for example `coord`.

```
sudo -u cb-headless mkdir -p -m 700 /Users/cb-headless/.config/crewboard/agents/<agent-dir>/keys
sudo install -o cb-headless -m 600 ~/.config/crewboard/agents/<agent-dir>/<initiative>.toml /Users/cb-headless/.config/crewboard/agents/<agent-dir>/<initiative>.toml
sudo install -o cb-headless -m 600 ~/.config/crewboard/agents/<agent-dir>/keys/<agent> /Users/cb-headless/.config/crewboard/agents/<agent-dir>/keys/<agent>
```

On Linux the home dir is `/home/cb-headless`. Put `headless = true` as the first line of the
copied `<initiative>.toml`. This key switches the headless scope on.

The env file with the crew token goes to the same user, mode 600. It must be the token of the
account that wrote the coordinator's join: the approval binds that account, and a message of the
coordinator counts only from it.

```
sudo install -o cb-headless -m 600 ~/.config/crewboard/<initiative>.env /Users/cb-headless/.config/crewboard/<initiative>.env
```

## 4. Keep the interactive config dir out of reach

The interactive coordinator runs as your own user with its own config dir. The headless user
must not read it, because it holds a signing key and has no scope. As the interactive user:

```
chmod 700 ~/.config/crewboard
chmod 700 ~/.config
```

Check it (the shell expands `~` to your own home before `sudo` runs):

```
sudo -u cb-headless /bin/ls ~/.config/crewboard
```

This must fail with "Permission denied".

## 5. Check

```
sudo -u cb-headless -H /usr/local/bin/crewboard doctor
```

The output must have `per-dir key: headless = true in <config dir>` and
`headless scope: per-dir key only (guardrail)`. Then check that an out-of-scope command is refused:

```
sudo -u cb-headless -H /usr/local/bin/crewboard assign 1 --to <agent>
```

It must print `refused [rule=headless-scope]` and exit 2, with no request to GitHub.

## 6. Run the wake as the headless user

The no-model coordinator starts no model. The scheduler runs `crewboard wake --tick`
as `cb-headless`. It runs `crewboard tick` under a lock and writes the tick lines to its log
(`~/.local/state/crewboard/agents/<initiative>-<agent-dir>/wake.log` of `cb-headless`). This is
the whole job:

```
/usr/local/bin/crewboard wake --tick --initiative <initiative> \
  --env-file /Users/cb-headless/.config/crewboard/<initiative>.env \
  --config-dir /Users/cb-headless/.config/crewboard/agents/<agent-dir>
```

Install the `crewboard` binary where the headless user cannot change it, for example
`/usr/local/bin/crewboard` owned by root (mode 755). On Linux put the line in the crontab of the user (`sudo crontab -u cb-headless -e`, with
`*/5 * * * * ` in front). On macOS use a LaunchDaemon in `/Library/LaunchDaemons` (owned by
root) with the key `UserName` set to `cb-headless` and `StartInterval` 300, so the headless user
cannot change the job. For a coordinator without a separate OS user,
`crewboard wake install --tick --agent-dir <dir>` writes and enables a user unit (see
[headless-claude.md](runtimes/headless-claude.md#headless-coordinator)).

## A `Gate: ci` merge has no human review

The no-model coordinator merges a ticket whose Gate is `ci` when the evidence, the protected
paths and a green CI run for the head SHA pass. No model and no person reads the diff. So a
`Gate: ci` ticket is as strong as the files that define CI: the workflow files, the test
config, the build file that names the test target, and the test files themselves. A dev agent
that edits one of them can make CI green without a test that means anything.

List these files in `protected_paths` in `AUTHORITY.md` ([Owner guide](owner.md#authoritymd)).
The built-in list holds only the authority file, `.crewboard.toml`, the CODEOWNERS files and the files
under `.github/workflows/` and `.github/actions/`. A pull request that touches a listed path
is merged by you only, also when the Gate is `ci`. On a headless host, `doctor --setup` warns
while `protected_paths` is empty.

## What the guardrail does not cover

The `headless` key limits what the `crewboard` binary does. It does not limit the
headless OS user:

- **The GitHub token.** The headless user holds a token of the crew account. A process of that
  user that runs its own code (`curl`, `gh`, a script) can call the GitHub API directly: merge
  a PR, edit a board field, post a comment. crewboard does not see this.
- **The signing key.** The headless user can read its key, so its own code can sign messages
  as the coordinator.
- **The model runtime.** `crewboard wake --tick` starts no model. If you start a
  model in a headless wake anyway, and its runtime lets the model run any shell command, the
  model can do both of the above. Then allow only `crewboard` commands (for example
  `--allowedTools "Bash(crewboard *)"` for Claude Code). This is a setting of the runtime, not
  a check of crewboard.

So limit the token as well. The token is the real power of the headless user:

- **Use a crew account** (the shared `crew_login` of `.crewboard.toml`, or an own account),
  never the owner's token. Give it
  write access only to the repos of the initiative, and no admin rights. An account without
  admin rights cannot change branch protection or repo settings.
- **Protect the main branch.** Require the CI status checks and a pull request before a merge,
  forbid force pushes, and do not allow anyone to bypass the rules. Then even a direct API
  merge needs green CI, which is the same condition that a headless `merge` checks.
- **No workflow rights.** A fine-grained token without the "Workflows" permission cannot change
  the CI files, so it cannot make CI green by editing the checks. Changes to workflow files go
  through the owner (see [Workflow files: the owner route](../design/spec/tickets.md#workflow-files-the-owner-route)).
- **Revoke on a doubt.** See "A key or a token leaks" in [security.md](security.md).

Why: crewboard protects the board from mistakes and from a model that only runs `crewboard`.
GitHub protects the repos from the token. A boundary against the model needs both.
