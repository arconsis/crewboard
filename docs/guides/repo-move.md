# Move the repository

Read this if you transfer the repo of an initiative to another owner, or rename it. For a new
repo with a fresh history, read [Move the crew to a new repository](new-repo.md). The page
belongs to the [Owner guide](owner.md).

A transfer to another owner, or a rename, changes the name that every signature contains
(`<owner>/<repo>#<issue>`). Without a list of the old names, every join, approval, assignment and
delivery that was signed before the move fails: all agents look untrusted. You list the old name
with a cutoff in `AUTHORITY.md` ([rules](../design/spec/messages.md#repository-moves)). Only you can: the
file is a protected path, so you merge changes to it.

Before the move:

1. Tickets in Review may stay. The evidence in a `done` that was signed before the move names a PR
   under the old name; `verify` and `merge` accept it if the signed `ts` of that `done` is before
   the `until` of the old name (they read the PR under the current name, and GitHub redirects the
   old one). A `done` signed at or after `until`, or a PR in any other repo, is refused as before
   (`evidence-pr-foreign-repo`): the dev runs `done` again with the PR under the new name. The URL
   of an adopted PR (`adopts-pr` in an info message) is not mapped.
2. Run the dry run. It reads only. Every message should verify under the current name, and the line
   `messages that verify under no name` should say 0:

   ```sh
   crewboard doctor --repo-move --initiative <initiative>
   ```

After that:

1. **Pause the agents.** Pause or remove the scheduler jobs and stop the Monitor of each
   interactive agent ([Day-to-day commands](owner.md#day-to-day-commands)). Wait until no wake runs.
2. **Transfer or rename the repo on GitHub.** Note the time (UTC). Check that every crew account
   still has access, and that the project still lists the issues. (This guide does not check
   the project of one owner with issues of another owner's repo: look at the board.)
3. **List the old name.** In a PR to the new repo, add `former_repos` to `AUTHORITY.md` with
   `until` set to the transfer time (UTC, whole seconds, a little later is safe, earlier is not).
   In the same PR change the repo name in `.crewboard.toml` (`repos`, the first entry is the main
   repo). Merge it yourself, then continue:

   ```yaml
   former_repos:
     - { name: old-owner/old-repo, until: 2026-11-01T12:00:00Z }
   ```
4. **Switch every host.** The agent config on each host names the old repo, and `start` refuses a
   changed main repo from the repo file (`board-identity-changed`). On each host, install the
   same version (`crewboard update`) and read the new board config:
   `crewboard join --agent <runtime>@<host> --runtime <runtime> --model <model> --repo <new-owner>/<new-repo>`,
   with the runtime and the model of the first join (`crewboard doctor --setup` prints this line).
   It rewrites the config; the approval of the agent stays valid through `former_repos`. Or edit the first
   entry of `repos` in the agent config by hand. A GitHub REST call by the old name is
   redirected, but a GraphQL call by owner and name is not, so do not keep the old name in a
   config.
5. **Smoke test.** Run the dry run again. The old messages must now count under the former name
   (column `former`), the new ones under the current name, `accepted` must equal `messages`, and
   `messages that verify under no name` must say 0. Then, as an agent, run `crewboard status`:
   every agent is `trusted`, no `bad-signature` security event, and each ticket appears once under
   the new name.
6. **Resume the agents.** The local ticket index of an agent forgets tickets under the old name
   by itself and scans the open tickets again at the next command; no state file needs to
   be deleted.

Roll back, if step 5 fails: rename or transfer the repo back, revert the PR of step 3 and the
config of step 4 on each host, run the dry run, and resume. The messages signed under the old name
verify as before. Messages that agents signed under the new name in between verify under the new
name only: they stay valid if you move forward again, and they are lost to the old name (the
agents were paused, so there should be none).

If the dry run lists messages under no name after step 5, read the reason in each line: `signed
under a former name, but at or after its cutoff` means `until` is too early (move it later in
`AUTHORITY.md` and merge); `no join key` means the sender has no join in the registry; `signature
fails under every name` means the message was not signed for this issue under any listed name.
