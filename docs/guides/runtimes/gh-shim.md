# The gh shim: once per agent

`gh` and `git push` in an agent's shell must act as the agent's account (the shared one or
the contributor's own: the account that the owner's approval bound), never as the owner.
`crewboard agent shim` makes sure of it (`crewboard agent add` does it for a new agent). Do it **once per agent**, for any runtime that
needs it.

## When you need it

You need the shim when ONE of these is true:

- The runtime strips `GH_TOKEN` and `GITHUB_TOKEN` from the commands that the model runs.
  Hermes does this. Codex may do it (its default environment policy drops names with `TOKEN`;
  not tested, see [codex.md](codex.md)).
- Plain `gh` on the host is logged in as the owner (`gh auth status`), so a `gh` without a
  token falls back to the owner. Its PRs and comments then carry the owner's name. A comment
  from the owner login without a header is an owner message to the coordinator. This is a
  security problem, not a style one.
- You do not know. `crewboard start` tells you: it refuses an agent whose plain `gh` is not its
  agent account (rule `agent-gh-not-crew`). That refusal means "install the shim". Its message prints the commands of the Steps below with your initiative and crew login filled in. Replace `<agent-dir>` and `<work-dir>`, then run them.

You can skip it when the runtime keeps `GH_TOKEN` and the env file is always loaded. Claude
Code does keep it ([unattended-claude.md](unattended-claude.md)). The shim is still a good safety
net: it also works when the env is lost.

## Steps

```sh
# the shim in the agent's own bin dir, an empty gh config dir next to it, and (with --work-dir) the
# git identity of the agent's checkout; a linked worktree keeps the owner's checkout untouched
crewboard agent shim --agent-dir ~/.config/crewboard/agents/<agent-dir> --work-dir <work-dir>
```

The command needs the env file of the agent account: `<agent-dir>/agent.env` (made by
`crewboard agent add`), or `--env-file <file>`. It finds the real `gh` as the first `gh` on the
PATH that is not the shim; `--real-gh <path>` names it. It reads the crew login and its id from
`<agent-dir>/crew-id` (written by `agent add`); `--crew-login <login>` names the login. It refuses
to overwrite a `bin/gh` that is not a crewboard stub. A second run says `already done`.

The shim is a file of three lines. Its last line runs
`crewboard agent with-env --env-file <agent-dir>/agent.env -- <real gh> "$@"`. It needs no `HOME` and no other variable: a runtime that strips the environment of the model's commands (Hermes) still gets the agent token for each `gh` call. If you move the
`crewboard` binary or install another `gh`, run `crewboard agent shim` again.

The agent shell needs the shim first on its `PATH` and an empty `GH_CONFIG_DIR`. The wake script
must contain them before the wake command; `crewboard wake install --shim-dir <agent bin dir>` does it for
you, also for Hermes ([hermes-cron.md](hermes-cron.md), step 1). For an interactive runtime, start
it with `crewboard agent exec --agent-dir <agent-dir> -- <runtime>`, or run
`eval "$(crewboard agent env --agent-dir <agent-dir>)"` in each new terminal before the runtime
starts: it prints the `PATH` and `GH_CONFIG_DIR` lines plus `CREWBOARD_CONFIG_DIR`.

What the shim does:

- It runs the real `gh` through `crewboard agent with-env`, so the agent token loads for that call only.
- A `gh` that bypasses the shim finds the empty config dir and fails ("not logged in").
- `--work-dir` points `credential.helper` at the shim, rewrites `git@github.com:` to HTTPS, and
  sets the author to the agent account (use the noreply address `<id>+<crew-login>@users.noreply.github.com`).

## The agent environment

The model of an agent calls plain `gh` and `git`. They must act as the agent account. Each agent
has its own agent dir. It holds the key, the agent config, `agent.env` (a link to the env file),
the shim `bin/gh` and `crew-id`. Two agents never share one dir. Run every agent command through
`crewboard agent`:

- `crewboard agent exec --agent-dir <agent-dir> -- <command>` runs any command, for example
  `crewboard status` or the runtime, with the shim first on the `PATH`, an empty `GH_CONFIG_DIR`
  and the config dir of the agent. crewboard never prints the token and ignores the token
  variables of the caller ([Token handling](../../design/spec/security-details.md#token-handling)).
- `eval "$(crewboard agent env --agent-dir <agent-dir>)"` does the same for a POSIX shell. It
  prints only `export` lines.
- `crewboard agent with-env --env-file <file> -- <command>` runs one command with the token of
  an env file.

Inside such a shell the config dir is set, so `crewboard status` needs no flag. From outside, use
`crewboard --config-dir <agent-dir> status`: it loads the `agent.env` of that dir.

One machine can host several agents, and can be the owner machine. Give each agent its own agent
dir and work dir. The agents share one env file. Run owner commands in a second terminal, never
in one with the agent environment.

## Check

Run these in the environment that the wake script gives the agent:

```sh
command -v gh                    # the shim
gh api user --jq .login          # the crew login
<real gh path> api user          # fails: not logged in
git -C <work-dir> config user.email   # the crew noreply address
```

Do not write "your token is in the environment" in a wake prompt for a runtime that strips it.
Tests: `cargo test --test component agent_shim`, and `bash skill/loops/tests/test-loops.sh` (cases G1 to G3 for
`agent shim`, A4 for `--shim-dir`).
