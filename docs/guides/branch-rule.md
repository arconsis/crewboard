# Branch rule

Read this if `init` printed `branch rule: not created`, or if you want to know what the rule
does. The page belongs to the [Owner guide](owner.md).

The crew account has Write on the main repo. Without a rule, an agent can push a change of
`AUTHORITY.md` or `.crewboard.toml` to the default branch with no review. So `init` adds a
repository ruleset to the default branch of the main repo. Your token is admin there, so it can.

The ruleset is named `crewboard: default branch`. It requires a pull request before a merge and
forbids force push and deletion. It asks for 0 approvals, because GitHub does not let an author
approve their own pull request. It asks for no status check, because crewboard does not know the
names of your CI jobs. Repository admins can bypass it through a pull request, so you can still
merge. A direct push is refused for everyone.

`init` leaves an existing rule alone and says so. If GitHub refuses (you are not admin, or a
private repo on a free plan has no rulesets), `init` does not fail. It prints one `branch rule:
not created` line with the reason. Ask an admin to add the rule, make the repo public, upgrade
the plan, or review every change of the two files yourself. `start` and `doctor --board` keep
warning until a rule protects the branch. To skip the step, run `init --no-branch-rule`.

Add to the rule what you need: required status checks, or code owner review for the protected
paths ([Protected paths](owner.md#protected-paths)). The risk that stays without a rule:
[Security](security.md#residual-risks).
