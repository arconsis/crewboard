# Quick try: one person, one machine

**This is a trial, not a crew.** It uses your own GitHub token, one machine and no crew
account. Only `init --insecure-one-account` makes a trial; without the flag, `init` makes a crew
where your token is refused for agents. Use it on a test repo only. For a real crew use [Getting started](getting-started.md).
These safety checks are off:

- Any token passes the agent commands, yours too. The rules `agent-uses-owner-token`,
  `approve-owner-account`, `agent-token-not-crew` and `agent-gh-not-crew` need a crew account.
- A comment from your account without a header counts as an owner message. An agent that
  forgets the header posts as you, and nobody can see who posted. `crewboard audit` lists such
  comments afterwards.

Step 1 writes a file that holds your full GitHub access. Keep it private and delete it
afterwards. New terms (initiative, coordinator, Gate, agent dir) are in the
[glossary](concepts.md#glossary).

One measured run (2026-10-27) took 18 min 54 s from the new repo to the merged pull request.

## What you need

- macOS or Linux and the tools in [Install](guides/install.md#what-you-need), with `gh` 2.17 or
  newer.
- crewboard and its skill ([Install](guides/install.md)).
- `gh` logged in as you, with the two scopes that `init --with-ci` needs:

  ```sh
  gh auth login
  gh auth refresh -h github.com -s project -s workflow
  ```

- Claude Code, logged in once (run `claude` and sign in). Only Claude Code works in this trial.
  Keep one terminal open for the coordinator.
- A test repo with a commit and a test. This makes one:

  ```sh
  gh repo create demo-app --private --clone && cd demo-app
  printf 'def add(a, b):\n    return a + b\n' > calc.py
  printf 'from calc import add\n\n\ndef test_add():\n    assert add(1, 2) == 3\n' > test_calc.py
  touch requirements.txt
  git add . && git commit -m "Start" && git push -u origin HEAD
  ```

In every command below, replace `you` with your GitHub login. `demo` is the name of the
initiative (one piece of work with one board). The owner commands (`init`, `approve`) use your
own `gh` login and ignore `GH_TOKEN` and `GITHUB_TOKEN`. Step 1 reads `gh auth token`, which
prints such a variable when one is set: run it in a shell without them.

## The steps

There are 9 steps, about 11 commands. Use one terminal for the owner commands and two more for
the two agents. If a step hangs or fails, see "If nothing moves" in
[Getting started](getting-started.md#the-first-ticket). The usual causes are a missing
`workflow` scope, a `--wait-approval` that does not end, and `claude` not on the `PATH`.

1. Store your token in an env file (mode 600). It does not show on the screen.
   `--insecure-one-account` lets the token keep its `workflow` scope:

   ```sh
   gh auth token | crewboard agent token --initiative demo --insecure-one-account
   ```

2. Make the board. This opens a pull request with `AUTHORITY.md`, `.crewboard.toml` and a CI
   workflow:

   ```sh
   crewboard init --initiative demo --repo you/demo-app \
     --insecure-one-account --with-ci --env-file ~/.config/crewboard/demo.env
   ```

3. Allow the coordinator to merge. On the init pull request (branch `crewboard/init-demo`) open `AUTHORITY.md`, click the pencil
   icon, change `coordinator_may_merge: false` to `true` and commit to the branch. Then merge
   the pull request.

4. Join both agents. Each command waits for your approval, so run each in its own terminal:

   ```sh
   crewboard agent add you/demo-app --env-file ~/.config/crewboard/demo.env \
     --agent-dir ~/.config/crewboard/agents/demo-coord --work-dir ~/crew/demo-coord \
     --agent claude@laptop --runtime claude --model sonnet --role coordinator --wait-approval
   ```

   ```sh
   crewboard agent add you/demo-app --env-file ~/.config/crewboard/demo.env \
     --agent-dir ~/.config/crewboard/agents/demo-dev1 --work-dir ~/crew/demo-dev1 \
     --agent claude-2@laptop --runtime claude --model sonnet --wait-approval \
     --install-wake --every 60s
   ```

   `--install-wake` installs a scheduler unit that wakes the dev agent when it has work. Here
   it wakes every 60 seconds, not every 5 minutes, to make the trial fast. On Linux, run the
   `next:` line that it prints. To let the dev agent run your test command itself, add
   `--allow 'Bash(<your tool> *)'` to the second command ([Agents guide](guides/agents.md#the-loop)).

5. Approve both, in the owner terminal:

   ```sh
   crewboard approve claude@laptop --role coordinator --initiative demo
   crewboard approve claude-2@laptop --initiative demo
   ```

   Both waiting commands continue. The coordinator command ends with a line to paste.

6. Paste that line in the coordinator terminal. It starts Claude Code as the coordinator, with
   its first prompt. Claude Code asks before each command: allow `crewboard *` for the session.

7. Give the coordinator a ticket. The **Gate** is the one line that decides whether the ticket
   is done, and it must be able to fail. Tell the coordinator in its chat:

   > Make a ticket "Add a Running the tests section to README.md" and assign it to
   > claude-2@laptop. Gate: CI is green and `grep -q '^## Running the tests' README.md` exits 0.

   The dev agent wakes within a minute, takes the ticket and opens a pull request. `done` may
   refuse with `unread-events`: the dev agent then runs `crewboard watch --timeout 1m` first.
   This is normal. To see the board, ask the coordinator, or run this in the owner terminal:

   ```sh
   crewboard agent exec --agent-dir ~/.config/crewboard/agents/demo-coord -- crewboard status
   ```

8. When the ticket is in Review, tell the coordinator: "Verify the ticket and merge it." The
   ticket is Done.

9. Stop the loop: `crewboard wake uninstall --agent-dir ~/.config/crewboard/agents/demo-dev1`.

## Next: a crew

Do not convert the trial board. Start a new initiative with
[Getting started](getting-started.md): a new initiative name and new agent dirs. A crew account
changes whose messages the agents trust: your approval binds each agent to the account that
wrote its join, and that is never yours.

**Clean up.**

- Delete the project and the repo on GitHub.
- Run `crewboard wake uninstall` for each unit.
- Delete `~/.config/crewboard/demo.*` and `~/.config/crewboard/agents/demo-*`.
- Delete `~/.local/state/crewboard/agents/demo-*` and `~/crew/demo-*`.
