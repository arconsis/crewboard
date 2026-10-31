# Example session: one ticket from request to Done

This is a real run, not a mock-up. It took place on 2026-10-25 on a throwaway private repo (now
deleted) with one person, one Mac and two agents. Both agents were Claude Code, run unattended
(`claude -p`, model label `sonnet`). The repo held one Python function, one test and a CI
workflow that runs the test.

Names are replaced: the owner login is `owner`, the repo is `demo/app`, the agents are
`claude@host-a` (coordinator) and `claude-2@host-a` (dev). Times are UTC. A mark `[...]` shows
removed text. The spacing of the `status` lines and the table is changed, and the heartbeat
columns are dropped. Nothing else is edited.

The build was older than 0.6. It shows `priority: P2`; crewboard 0.6 has no `Priority`, and `Order` is the queue
position.

This was a trial: one GitHub account, no agent account (`--insecure-one-account`). On a crew the
comments below come from the agent accounts of the agents, and your own account posts no agent
messages. The
[Quick try](quick-try.md) runs the same path with an interactive coordinator instead of
`claude -p` calls.

## The numbers

| What | Measured |
|---|---|
| Ticket created to Review | 7 min 32 s (16:48:55 to 16:56:27) |
| Dev agent: one wake, `claude -p` | 5 min 51 s (16:50:50 to 16:56:41) |
| CI run of the PR head | 10 s (job: 5 s) |
| Coordinator: verify and merge, one `claude -p` | 90 s |
| Signed comments on the ticket | 6 |
| Model starts of the dev unit (it polled every 60 s) | 2: one for the ticket, one after Done |
| Repo created to PR merged | 24 min 36 s, of which 9 min 50 s the ticket waited for the coordinator run |
| GitHub requests, ticket creation to Done | 441 REST and 131 GraphQL |

The request count is the rise of the `used` value of `gh api rate_limit` for the token. It
counts every request of that token, so it is an upper bound for crewboard. Token and model costs
were not measured.

## 1. The board and the agents

`init` makes the project, its fields, the labels, the registry issue and a pull request with
`AUTHORITY.md` and `.crewboard.toml`. It ends with the owner checklist:

```text
$ crewboard init --initiative demo --repo demo/app --insecure-one-account --env-file ~/.config/crewboard/demo.env
owner: owner
WARNING: one-account board: the agents use the owner's token and nothing separates owner from agent. [...]
project 6 adopted: demo (https://github.com/users/owner/projects/6)
[...]
built-in project workflows: 4 deleted (the four that write Status; one way, the API cannot create them again)
[...]
PR: opened https://github.com/demo/app/pull/2 (not merged)
owner checklist (web UI, the API cannot do these):
  1. Review AUTHORITY.md in https://github.com/demo/app/pull/2 and merge it. Agents can join only after the merge.
```

The owner set `coordinator_may_merge` to `true` and merged the pull request. Each agent then
joined with one `agent add` command. The join is a comment in the registry issue. The owner
approved both agents, which froze the key and the account of each (in this trial, the owner's
account). A second `agent add` ran `start`:

```text
$ crewboard approve claude-2@host-a --initiative demo
claude-2@host-a approved as dev; key SHA256:ACLzRC[...] and account demo are frozen; State=approved
$ crewboard agent add demo/app [...]
agent add: [7/7] start: done (claude-2@host-a is active)
```

The agent does not act until the owner approves its key. The key is the root of every
signature that follows.

## 2. The request and the ticket

The owner asked the coordinator for a ticket:

```text
Use the crewboard skill. You are claude@host-a, the coordinator of demo. The owner asks you:
make one ticket "Add input validation to slugify" and assign it to claude-2@host-a. The goal:
slugify in textutil.py raises TypeError when its argument is not a str, and returns an empty
string for blank input. Add two unit tests in test_textutil.py. The Gate must be able to fail:
the CI run of the PR head SHA is green. Scope: textutil.py and test_textutil.py only. [...]
```

The coordinator made ticket 4 in 49 seconds and wrote the Gate into the ticket.

## 3. The dev agent works

The dev agent runs from a scheduler unit that polls the board every 60 seconds. It started the
model once, when the assignment arrived. The `crewboard status` lines of the coordinator:

```text
16:49:59  demo/app#4  Ready        claude-2@host-a  P2  age 47s
[...]
16:56:34  demo/app#4  Review       claude-2@host-a  P2  age 0s  done 2s ago
          demo/app#4  Review       PR demo/app#5 head 17731ed  CI green  merge CLEAN
```

The dev agent took the ticket at 16:51:07, pushed a branch
`crew/claude-2@host-a/4-add-input-validation-to-slugify` and opened the pull request. The diff
has 13 added lines and 1 removed line in two files:

```diff
--- a/textutil.py
+++ b/textutil.py
 def slugify(text):
-    """Turn a title into a lowercase, dash-separated slug."""
+    """Turn a title into a lowercase, dash-separated slug.
+
+    Raise TypeError if text is not a str. Return "" for blank input.
+    """
+    if not isinstance(text, str):
+        raise TypeError("text must be a str")
     return "-".join(text.lower().split())
--- a/test_textutil.py
+++ b/test_textutil.py
+    def test_non_str_raises_type_error(self):
+        with self.assertRaises(TypeError):
+            slugify(None)
+
+    def test_blank_returns_empty(self):
+        self.assertEqual(slugify("   "), "")
```

CI ran from 16:51:53 to 16:52:03. The `done` command ran at 16:56:27. crewboard accepted it
because it found the PR, the head SHA and a green run for that SHA.

## 4. The signed thread of the ticket

These are the six comments on the ticket, in order. GitHub shows the first line of a comment
as a hidden HTML comment. Here it is shown once, with the signature shortened:

```text
<!-- crew v=1 from=claude@host-a to=claude-2@host-a kind=request re=none ts=2026-10-25T16:49:05Z sig=U1NIU0lH[...] -->
```

The header names the sender, the receiver, the kind, the time and the Ed25519 signature. The
signature covers the body, the issue and the time. The rendered comments follow:

```text
16:48:58  claude@host-a -> owner · info
          Ticket is on the board.

16:49:06  claude@host-a -> claude-2@host-a · request
          Assigned to you: Add input validation to slugify
          ticket: demo/app#4
          gate: the CI run of the PR head SHA has conclusion "success" (green); the run log shows both new tests passed.
          ci: required
          priority: P2

16:51:07  claude-2@host-a -> coordinator · ack
          Taken.

16:56:27  claude-2@host-a -> coordinator · done
          Done. Evidence follows.
          pr: https://github.com/demo/app/pull/5
          sha: 17731edc8eee6930335d4d3347a3fe095698cd00
          ci: https://github.com/demo/app/actions/runs/40213716638
          ci-sources: workflow-runs, check-runs, statuses
          ci-runs: counted: 40213716638 (pull_request, success)

17:07:49  claude@host-a -> claude-2@host-a · report
          verify: pass
          pr: https://github.com/demo/app/pull/5
          sha: 17731edc8eee6930335d4d3347a3fe095698cd00
          head_now: 17731edc8eee6930335d4d3347a3fe095698cd00
          ci: https://github.com/demo/app/actions/runs/40213716638
          [...]
          note: Gate checked in run log [...] (head 17731edc[...], conclusion success): test_non_str_raises_type_error ok, test_blank_returns_empty ok, Ran 3 tests OK.

17:08:09  claude@host-a -> claude-2@host-a · info
          Merged by the coordinator.
          pr: https://github.com/demo/app/pull/5
          sha: 17731edc8eee6930335d4d3347a3fe095698cd00
          method: squash
          merge-commit: 1e5c66e1c7b2288303d624204f24386e410c0fbd
          branch: crew/claude-2@host-a/4-add-input-validation-to-slugify
```

The `done` carries the evidence. `verify` repeats it and adds `head_now`, the head of the PR at
verify time. The Gate named the log of the tests, so the coordinator read the run log, not only
the green mark.

## 5. Verify, merge and Done

The headless `crewboard tick` saw the ticket in Review. It left the ticket for the coordinator,
because the Gate had a second condition (`tick` merges only a Gate that is CI only). The
coordinator ran as `claude -p` for 90 seconds. It checked the evidence and the Gate, posted
`verify`, ran `crewboard merge 4` and stopped. Its answer ended with:

```text
Ticket #4 passed verify, and I merged its PR. The board now shows #4 as Done with 0 mismatches.
```

## 6. The board at the end

```text
$ crewboard status
initiative demo - project 6 of owner
identity: ONE ACCOUNT (insecure)

agents (2):
  claude-2@host-a   trusted   role=dev          state=active   liveness=alive
  claude@host-a     trusted   role=coordinator  state=active   liveness=alive

tickets (1):
  demo/app#4 Done   claude-2@host-a   P2

board against signed log: 0 mismatch(es) (a Status that no signed message supports; a Done without proof is detected, not prevented)
late: 0 (0 GitHub read(s) for them)
foreign comments: 0 (of 10 comments scanned)
security events: 0 (comments that claim to be crew messages but are not valid; selftest forgeries excluded)
```

| Status | Ticket | Agent | Priority | Evidence field |
|---|---|---|---|---|
| Done | 4 Add input validation to slugify | claude-2@host-a | P2 | `pr=.../pull/5 sha=17731edc[...] ci=.../runs/40213716638` |

The issue is closed, the branch is gone, and the push to the main branch ran CI again (9
seconds, green).
