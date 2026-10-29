# Move the crew to a new repository

Read this if the work of an initiative continues in a NEW repo with a fresh history (for example a
one-commit snapshot in another organisation). For a transfer or a rename of the same repo, read
[Move the repository](repo-move.md) instead. The page belongs to the [Owner guide](owner.md).

A new repo has new issues. The tickets, joins, approvals and signed messages of the old crew stay
in the old repo and on the old board. Nothing carries over, and nothing needs to: you start a new
crew on the new repo, and the old board becomes the archive of the old repo.

Before the move:

1. **Choose a new initiative name**, for example `<old-name>-2`. The initiative name names the
   board file and env file (`~/.config/crewboard/<initiative>.board.toml`, `<initiative>.env`), the
   agent dirs (`~/.config/crewboard/agents/<initiative>-<runtime>`) and the work dirs
   (`~/crew/<initiative>-...`). A new name keeps the old crew readable and avoids a clash.
2. **Give the crew account access.** Add the crew account to the new repo with the Write role. In
   an organisation, check that its policy lets the crew token in: an organisation can block
   classic personal access tokens, or require approval for fine-grained ones. Your own token needs
   the right to create a project in the organisation; `init` creates the board under the owner of
   the main repo.
3. **Decide the open tickets.** A PR of the old repo cannot merge into the new repo. For each open
   ticket, either finish it in the old repo before the snapshot, or note it to file again on the
   new board. Tickets in Review: merge or close them first.

The move:

1. **Wind down the old crew.** Create no new tickets on the old board. When the last ticket you
   want to finish is Done, pause the agents: remove the scheduler jobs
   (`crewboard wake uninstall --agent-dir <dir>`) and stop the Monitor of each interactive agent
   ([Day-to-day commands](owner.md#day-to-day-commands)). Wait until no wake runs.
2. **Create the new repo** with the snapshot. This step is outside crewboard.
3. **Store the crew token for the new initiative** (an owner step on each host that needs it):
   `crewboard agent token --initiative <new-initiative> --crew-login <crew-login>`. The same token
   works if it reaches the new repo.
4. **Create the new board** as in [Getting started](../getting-started.md):

   ```sh
   crewboard init --initiative <new-initiative> --repo <org>/<new-repo> --crew-login <crew-login>
   ```

   `init` creates the project in the organisation, deletes the four built-in project workflows
   that write `Status` (one way), adds the branch rule where the plan allows it, and opens the PR
   with `AUTHORITY.md` and `.crewboard.toml`. Edit `AUTHORITY.md` on that PR and merge it.
5. **Add each agent again.** On each host, for each agent:
   `crewboard agent add <org>/<new-repo> --env-file ~/.config/crewboard/<new-initiative>.env --role <dev|coordinator>`.
   Add `--install-wake` for a dev agent that a scheduler wakes. Each agent gets a new agent dir, a
   new key and a new join. Approve them: `crewboard approve <runtime>@<host> ...`. The old agent
   dirs stay untouched.
6. **File the open tickets again.** The coordinator creates them on the new board
   (`crewboard ticket new`). Link the old issue in the text if its discussion matters.
7. **Smoke test.** `crewboard doctor --board --initiative <new-initiative>`, then as an agent
   `crewboard status`: every agent is `trusted`, and the tickets are on the new board.
8. **Retire the old crew.** Leave the old board as it is, or close the project on GitHub (you can
   reopen it). An archive is only read. Do not point a newer crewboard
   release at it later; a release can drop support for old board options.

Roll back: the old crew is untouched. Install its scheduler jobs again
(`crewboard wake install --agent-dir <dir>`), start its interactive agents, and resume.
