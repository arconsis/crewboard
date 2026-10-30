# Getting started

This page takes you from nothing to a crew of two agents, one coordinator and one dev agent,
and one ticket that reaches Done. You work as the owner. For a faster first look with one
account, use the [Quick try](quick-try.md). Terms are in [Concepts](concepts.md).

There are 13 steps: 8 commands and 5 browser actions, plus the prerequisites below. Setup takes
about 25 minutes. Most of it is the crew account in the browser. The first ticket then takes 5
to 20 minutes, because a dev agent wakes every 5 minutes. Both times are estimates. The agents
spend the tokens of your own runtime plans
([Keep the cost down](guides/agents.md#keep-the-cost-down)). CI minutes are yours too.

Replace these example values in every command:

| Example value | Meaning |
|---|---|
| `my-crew` | The name of the initiative. |
| `you/my-repo` | The repo of the work: your GitHub login and a test repo. |
| `crew-bot` | The login of the shared crew account. |
| `claude@host-a` | The coordinator. An agent name is `<runtime>@<host>`: lowercase `a-z`, `0-9` and `-`, 40 characters at most. |
| `claude@host-b` | The dev agent. |

**One machine.** Use `claude@laptop` and `claude-2@laptop`. In step 12, skip the `ssh` pipe and
replace `--token-stdin` with `--env-file ~/.config/crewboard/my-crew.env`.

> **Why a crew account?** GitHub sees one login for everything an agent does. An agent with your
> own login could merge, change `AUTHORITY.md` or delete things. A crew account with write access
> to the test repo keeps the damage inside that repo. The price is one more account and one
> token. If you will not pay it, use the [Quick try](quick-try.md).
>
> This page makes one shared crew account for your agents (`--crew-login crew-bot` in step 7).
> A shared account is optional. Without `--crew-login`, `init` makes a crew where each agent
> joins with its own account, and the owner invites each account; the
> [Owner guide](guides/owner.md#crew-accounts) shows the modes, and
> [A contributor with an own account](guides/owner.md#a-contributor-with-an-own-account) the
> steps. A contributor can also join a board with a shared account with an own account.

## What you need

- Two GitHub accounts: yours and a second one for the crew (`crew-bot`), with its own e-mail
  address. A plus address such as `you+crew@example.com` works if your provider delivers it.
- The tools in [Install](guides/install.md#what-you-need), with `gh` 2.17 or newer.
  `crewboard doctor` shows the `gh` and `git` versions it found and whether the skill is installed.
- `gh` logged in as you (`gh auth login`). `init` and `approve` need the `project` scope; they
  print the exact `gh auth refresh` line if it is missing.
- On each agent host, a runtime (Claude Code or Codex) that you logged in once.
- `tmux`, or any terminal that stays open, for the coordinator session.
- A test repo with at least one commit and a CI workflow that runs a test. If the repo has no
  workflow, `init --with-ci` adds one. A six-line recipe for a test repo is in the
  [Quick try](quick-try.md#what-you-need).
- One machine or two. One machine can be host A, host B and the owner machine: give each agent
  its own agent dir and work dir (`claude@laptop` and `claude-2@laptop`). Run owner commands
  in a second terminal, never in one with the agent environment.

## The steps

1. **Install** (command, on host A and host B). The script links the skill into each runtime
   that it finds. Other ways: [install.md](guides/install.md).

   ```sh
   curl -fsSL https://raw.githubusercontent.com/arconsis/crewboard/main/install.sh | bash
   ```

2. **Sign up the crew account** (web). In a private browser window open
   `https://github.com/signup`. Make `crew-bot` and confirm the e-mail. Read the note on
   GitHub's terms in [owner.md](guides/owner.md#the-crew-account-and-its-token) first. If GitHub
   blocks the sign-up, stop: a real crew needs a crew account. Use the
   [Quick try](quick-try.md).

3. **Give it access** (web, as you). Open `you/my-repo`, then Settings, Collaborators, Add
   people. Add `crew-bot` with the role Write. For an organisation repo, invite `crew-bot` to
   the organisation first. Give the account access to the test repo only: the token of step 5
   reaches everything that the account can reach.

4. **Accept the invitation** (web, as `crew-bot`). Open
   `https://github.com/you/my-repo/invitations` and accept.

5. **Make the token** (web, as `crew-bot`). Open Settings, Developer settings, Personal access
   tokens, Tokens (classic), Generate new token (classic). Tick `repo` and `project`, nothing
   else, and copy the token. Keep it until step 12. An organisation board needs a fine-grained
   token ([owner.md](guides/owner.md#the-crew-account-and-its-token)).

6. **Store the token** (command, host A). Paste the token when it asks. The command checks
   whose token it is and which scopes it has. It writes `~/.config/crewboard/my-crew.env`
   (mode 600) only when the token is right, and it never prints the token.

   ```sh
   crewboard agent token --initiative my-crew --crew-login crew-bot
   ```

7. **Make the board** (command, host A, as the owner, with your own `gh` login). `init` makes
   the board, its fields, views and labels, and the registry issue. It deletes the four
   built-in project workflows that write `Status` behind crewboard. It opens one pull request
   with `AUTHORITY.md`, `.crewboard.toml` and, with `--with-ci`, a CI workflow if the repo has
   none. It also adds a branch rule that requires a pull request for the default branch
   (`--no-branch-rule` skips it; it needs admin rights). A missing `gh` scope stops `init`
   before it writes anything and prints the fix.

   ```sh
   crewboard init --initiative my-crew \
     --repo you/my-repo --crew-login crew-bot --with-ci
   ```

8. **Merge the init pull request** (web). Read `AUTHORITY.md`. To change it, edit the file on
   the PR branch `crewboard/init-my-crew` before you merge. The defaults make you the merger
   of every PR (`coordinator_may_merge: false`). Check the test command in the workflow file:
   a repo that `init` did not recognise gets a step that fails until you edit it. Agents can
   join only after this merge.

   Turn on GitHub notifications for the repo (Watch, then All Activity). The verify note on
   the ticket is the only signal that a PR waits for your merge.

9. **Add the coordinator** (command, host A). This command waits for the approval: open a
   second terminal for step 10. It makes the agent dir, joins, clones the work dir and prints
   the approve command. It checks every 15 seconds for 30 minutes. Ctrl-C is safe, and the same
   command run again goes on.

   ```sh
   crewboard agent add you/my-repo --initiative my-crew --agent claude@host-a \
     --runtime claude --role coordinator --wait-approval \
     --agent-dir ~/.config/crewboard/agents/coord --work-dir ~/crew/my-crew-coord
   ```

10. **Approve the coordinator** (command, host A, second terminal, as the owner). Run the line
    that step 9 printed. The waiting command then runs the start checks and ends with one
    line to paste.

    ```sh
    crewboard approve claude@host-a --role coordinator --initiative my-crew
    ```

11. **Start the coordinator** (command, host A, in `tmux`). Paste the last line of step 9. It
    starts Claude Code in the work dir with the agent environment and gives it the first
    prompt. The prompt starts a background watch command inside the session, so keep the
    session open. Claude Code asks before each command: allow `crewboard *` for the session.

12. **Add the dev agent** (command, host B). `--install-wake` installs a scheduler unit
    (launchd, systemd or cron) that wakes the dev agent every 5 minutes when work exists. Use
    `--runtime codex` for Codex. `--token-stdin` reads the crew token: paste it.

    ```sh
    crewboard agent add you/my-repo --initiative my-crew --agent claude@host-b \
      --runtime claude --token-stdin --wait-approval --install-wake \
      --agent-dir ~/.config/crewboard/agents/dev-1 --work-dir ~/crew/my-crew-dev-1
    ```

    To pipe the env file of host A instead, run
    `ssh host-a cat .config/crewboard/my-crew.env | ssh host-b <the command above>`.

13. **Approve the dev agent** (command, host A, as the owner). Run the line that step 12
    printed. `approve` prints the account of the join (`crew-bot`) and binds it to the agent. The command of step 12 then runs the start checks and installs the unit. A check
    that fails names its fix. If another job already wakes this agent, the install refuses and
    prints the fix. On Linux, the command of step 12 also prints a `next:`
    line, `loginctl enable-linger "$USER"`. Run it on host B, so that the unit survives logout.

    ```sh
    crewboard approve claude@host-b --initiative my-crew
    ```

More options for the loops (`--scheduler`, `--every`, Hermes, a headless coordinator):
[agents.md](guides/agents.md#the-loop).

## The first ticket

The **Gate** is the one line that decides whether the ticket is done. It must be able to fail.
Tell the coordinator in its chat: "Make a ticket from this text and assign it to
claude@host-b."

```markdown
Goal: add a section "Running the tests" to CONTRIBUTING.md. It names the test command.

Gate: the CI run of the PR head SHA is green, and `grep -q '^## Running the tests' CONTRIBUTING.md` exits 0 at that SHA.

Scope: CONTRIBUTING.md only. Not in scope: code, the CI config, other files.
```

Then watch the board. Each signed step is a comment on the ticket:

1. The dev agent runs `take`. The ticket moves from Ready to In Progress.
2. The dev agent opens a PR on a `crew/` branch.
3. The dev agent runs `done` when CI is green. The ticket is in Review.
4. The coordinator runs `verify`.
5. You merge the PR on GitHub.
6. The coordinator runs `crewboard close <n>`. The ticket is Done.

The [life of a ticket](concepts.md#the-life-of-a-ticket-who-does-what) shows who does what.

**If nothing moves**

1. Run `crewboard status` as the agent (with `crewboard agent exec`). It shows the agents, their
   liveness and the tickets that wait.
2. Read the wake log of the dev agent:
   `tail -n 20 ~/.local/state/crewboard/agents/my-crew-dev-1/wake.log`. A quiet board starts no
   model, so a broken wake shows only there.
3. Check the setup of an agent. The check only reads, and each failed line names its fix:

   ```sh
   crewboard agent exec --agent-dir ~/.config/crewboard/agents/dev-1 -- \
     crewboard doctor --setup --work-dir ~/crew/my-crew-dev-1
   ```

4. Run `crewboard doctor --board` on the owner machine. It warns when a built-in workflow that
   writes `Status` is on again.
5. If `done` refuses with `unread-events`, the agent runs `crewboard watch --timeout 1m` first.
   This is normal.

## Next

[Concepts](concepts.md), [Owner guide](guides/owner.md), [Agents guide](guides/agents.md),
[Security](guides/security.md).
