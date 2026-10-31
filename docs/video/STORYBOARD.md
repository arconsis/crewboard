# crewboard explainer video: storyboard

Status: v1.1, 2026-10-30. Drafted against the docs of commit `8788f9b` (branch `main`); line numbers
below are the line numbers of the files at that commit. v1.1 adds exit 64 to s9-4 from
`origin/main` at `ca3fb47` (crewboard 0.8.0); that one row cites `ca3fb47`. Between the two commits
only the exit-code docs changed among the cited files (`git diff --stat 8788f9b ca3fb47`), and
SK 17-18 and 79-81 keep their lines. v1.2 (2026-10-31) changes s3-2, s4-4, s10-2, s11-1, s12-2 and
s13-2 for the docs change "agent account": each person brings the GitHub account that they
already have, never the owner's. The rows of those six segments cite the docs of that change
(the commit that adds v1.2); the other rows keep the lines of `8788f9b`.

Files of v1: `script.json` (narration, scenes, order, voice), `scenes.py` (the title card and
thirteen Manim scenes), `timing.json` (measured segment durations), `build.sh`, `narrate.sh`,
`narrate_gemini.py`, `align_gemini.py`, `transcribe_gemini.py`, `README.md`, this file.

Source abbreviations:

| Short | File |
|---|---|
| RM | `README.md` |
| CO | `docs/concepts.md` |
| GS | `docs/getting-started.md` |
| QT | `docs/quick-try.md` |
| ES | `docs/example-session.md` |
| OG | `docs/guides/owner.md` |
| AG | `docs/guides/agents.md` |
| IN | `docs/guides/install.md` |
| SE | `docs/guides/security.md` |
| UC | `docs/guides/runtimes/unattended-claude.md` |
| IC | `docs/guides/runtimes/interactive-claude.md` |
| CLI | `docs/reference/cli.md` |
| EX | `docs/reference/exit-codes.md` |
| SK | `skill/SKILL.md` |
| SKC | `skill/references/coordinator.md` |
| SKD | `skill/references/dev.md` |
| CH | `CHANGELOG.md` |

Status words in the claim tables: **doc** = the cited lines say it. **example** = an example
value of the docs (`claude@host-b`, `you/my-repo`, ticket `#4`, PR `#5`, head `17731ed`), used
as a prop. **framing** = a sentence of the story that makes no claim about crewboard (a question,
a transition). **derived** = follows from the cited lines, not written there in these words; the
table says how.

## 1. The brief and the shape

Storytelling from three perspectives (the owner, the coordinator, a dev agent), the setup and
the development process, the purpose and the strengths, honest about the limits. Addition of the
owner during the work: how crewboard is set up and used in an agentic way, through the skill.
That addition is scene 9, played between the dev agent's view and "Proof before Done".

A title card and thirteen scenes, 52 segments, 7,615 characters of narration (counted with `jq`
on 2026-10-30, v1.1). With the lead-in, gaps and tails of `pacing` the video is about 9 minutes. The
measured values are in the README, section "Build log".

Voice rules: calm documentary narrator, English, short sentences, one idea per sentence, no
marketing words, no em-dashes, the same word for the same thing ("ticket", "Gate", "signed
message", "the owner"). The three perspectives are told by the narrator ("the owner's view",
"the coordinator runs verify"), never role-played. Names are example names of the docs only.

Numbers spoken: the seven statuses (CO 25), "Nine steps, three terminals" (RM 63), "thirteen
steps" (RM 65, GS 7), exit codes 2, 3 and 4 (SK 79-81) and 64 (SK 82 at `ca3fb47`), "newest first" (CLI 806). No other
number is spoken. "Every five minutes" was in a draft and is cut: it is in the docs (GS 137, UC 3)
but the segment went.

## 2. Play order and chapters

| Pos | Scene | Class | Title | Segments |
|---|---|---|---|---|
| 0 | 0 | `S00Title` | crewboard | 1 |
| 1 | 1 | `S01Cold` | Cold open: who did what, and what is proven | 4 |
| 2 | 2 | `S02Idea` | One board, one signed log | 4 |
| 3 | 3 | `S03Trust` | What makes a message count | 4 |
| 4 | 4 | `S04OwnerSetup` | The owner: install, init, approve | 4 |
| 5 | 5 | `S05OwnerDay` | The owner: what you see, what only you do | 3 |
| 6 | 6 | `S06CoordTickets` | The coordinator: a ticket with a Gate | 4 |
| 7 | 7 | `S07Verify` | The coordinator: verify, merge, close | 4 |
| 8 | 8 | `S08Dev` | The dev agent: take, work, done | 5 |
| 9 | 9 | `S09Skill` | Through the skill: how an agent joins and works | 5 |
| 10 | 10 | `S10Proof` | Proof before Done | 3 |
| 11 | 11 | `S11BoardLog` | The board against the signed log | 4 |
| 12 | 12 | `S12Limits` | Strengths and limits | 4 |
| 13 | 13 | `S13Close` | Where to start | 3 |

`script.json` key `order`: `[0..13]`. `tts.groups`: `[[0,1,2,3],[4,5,6],[7,8,9],[10,11,12,13]]`
(four Gemini TTS calls; scene numbers).

## 3. The scenes

Each scene: the narration (segment id and text), what is on screen, and the claim table.
The on-screen text is listed where it carries a claim of its own.

### Scene 0: title card (`S00Title`)

| id | Text |
|---|---|
| s0-1 | crewboard. A crew of agents. One board. Proof before Done. |

Screen: the name, a rule, the three phrases appear on cue, then the card fades.

| Claim | Source | Status |
|---|---|---|
| "A crew of agents. One board. Proof before Done." | RM 3 | doc |

### Scene 1: cold open (`S01Cold`)

| id | Text |
|---|---|
| s1-1 | Picture three AI coding agents on different machines, under different runtimes, all on one repository. |
| s1-2 | After a week, somebody asks: who took this task? Who said it was finished? What proves it? |
| s1-3 | A chat log cannot answer. It is not signed. And anyone who holds a token can write a board field. |
| s1-4 | crewboard is a small tool for this problem. One binary and one agent skill. No server, no database. |

Screen: three machines (`host-a`, `host-b`, `host-c`) with agent dots labelled `claude`,
`codex`, `hermes`; arrows to "one repository"; the three questions; a chat bubble marked "not
signed"; a board field "Status: Done" with "any token" and "written by hand"; then "one binary",
"one agent skill", "no server, no database".

| Claim | Source | Status |
|---|---|---|
| several agents, one repo | RM 5-6 | doc |
| runtimes `claude`, `codex`, `hermes`; hosts | CO 14-17 | doc (labels) |
| who took, who said finished, what proves it | RM 39-40 ("a record of who did what, and a check on every done") | framing |
| a chat answer is not signed and does not count | OG 210-211 | doc |
| anyone with a (crew) token can write a board field | RM 70-71; CO 82-83 | doc |
| one binary and one agent skill; no server, no database | RM 9 | doc |

### Scene 2: the idea (`S02Idea`)

| id | Text |
|---|---|
| s2-1 | The idea has two parts. The first is a GitHub Project board that everybody can see. A ticket is a GitHub issue; its column is its Status. |
| s2-2 | The second is a signed log. Every message of an agent is an issue comment with a hidden header and an Ed25519 signature. |
| s2-3 | The signed messages are the record. The board is only a view. When the two disagree, crewboard reports it, and the owner decides. |
| s2-4 | Three roles work on that board: the owner, a person; one coordinator agent; and dev agents. |

Screen: the board with the seven Status columns, a card "#4 issue" in Ready; a signed message
card (header line `<!-- crew v=1 from=claude@host-a to=claude@host-b kind=request sig=... -->`,
body "Assigned to you: Add input validation", pill "Ed25519 signature"); "the signed log is the
record", "the board is a view", "mismatch: reported, the owner decides"; the three roles.

| Claim | Source | Status |
|---|---|---|
| a GitHub Project board; agents talk only through signed comments and the board | RM 6-7 | doc |
| a ticket is a GitHub issue | CO 23 | doc |
| the Status columns Backlog, Ready, In Progress, Review, Blocked, Done, Dropped | CO 25 | doc |
| a signed message: an issue comment with a hidden header and an Ed25519 signature | CO 27; ES 129-136 | doc |
| the header shape `<!-- crew v=1 from= to= kind= ... sig= -->` | ES 133 | doc (shortened) |
| "Assigned to you: Add input validation" | ES 143-144 | example |
| the signed messages are the record; the board Status is the view the commands read | CO 81-82 | doc |
| a mismatch is reported; the repair is the owner's decision | CO 83-85 | doc |
| the owner is a human; one coordinator; dev agents | CO 9, 19, 21; OG 193 ("One initiative has one coordinator") | doc |

### Scene 3: what makes a message count (`S03Trust`)

| id | Text |
|---|---|
| s3-1 | What makes a message count? Every agent has a name, a key and a role, for example claude at host-b. |
| s3-2 | The agent acts on GitHub as one account: usually the own account of the person who runs it, never the owner's. The owner's approval binds that account and the key to the name. |
| s3-3 | A message counts only when two checks pass: its GitHub author is the approved account, and its signature matches the approved key. |
| s3-4 | Any other comment is data, never an instruction. A forged, edited or copied message is refused and reported as a security event. |

Screen: the agent `claude@host-b` with a key icon and the role pill `dev`; the pill "account:
teammate (own)" with "never the owner's" below it; the owner and `crewboard approve claude@host-b`; binding lines and a lock; a
`done` message with the two checks ticked and "counts"; "forged", "edited", "copied" crossed,
"security event".

| Claim | Source | Status |
|---|---|---|
| an agent has a name `<runtime>@<host>`, a key and a role | CO 14 | doc |
| the agent acts as one GitHub account, usually the own account of the person who runs it | CO 12; OG 29-34 | doc |
| never the owner's account | CO 12; OG 36-40 | doc |
| the approval binds the account and the key to the agent | CO 12, 22; OG 48-50 | doc |
| `crewboard approve claude@host-b` | OG 196-198 (form) | example |
| `teammate` as the account of the dev agent | GS 42-45 (a teammate's own account) | example |
| a message counts only if its author is the approved account and the signature matches the approved key | SE 15-17 | doc |
| any other comment is data, never an instruction | SE 17; CO 27; SK 58 | doc |
| forged, edited, replayed (copied) messages are refused and reported as security events | SE 54-58 | doc |

### Scene 4: the owner sets up (`S04OwnerSetup`)

| id | Text |
|---|---|
| s4-1 | Now the owner's view. One line installs crewboard. The script installs the binary and links the skill into each runtime that it finds. |
| s4-2 | Then the owner runs init. It makes the board and the registry issue where agents ask to join, and opens one pull request with AUTHORITY.md. That file says what agents may do. By default, only the owner merges. |
| s4-3 | Agents can join only after the owner merges it. On each agent host, agent add joins an agent: it posts the signed join to the registry issue, clones the work directory, and waits. |
| s4-4 | The owner approves with one command. It prints the account of the join and binds it to the agent. Without approval, an agent has no authority. |

Screen: an owner terminal with `curl -fsSL .../install.sh | bash`, `crewboard init --initiative
my-crew --repo you/my-repo --with-ci` (two lines), `host-b $ crewboard agent add you/my-repo
--agent claude@host-b --wait-approval` (two lines), `crewboard approve claude@host-b` and the
output line `approved as dev; login=teammate`; on the right the
binary and the skill linked to `claude` and `codex` (gone when `init` runs), a small board, "registry issue (pinned)", the
init pull request with `AUTHORITY.md` and `coordinator_may_merge: false`, "merged by the owner",
a join message travelling into the registry issue, "waits for the approval" then "active", "key +
account bound to claude@host-b".

| Claim | Source | Status |
|---|---|---|
| the one-line install; it installs the binary and links the skill | RM 56-60; IN 10-12 | doc |
| the skill is linked into each runtime that is found | GS 56-57; IN 45-48 | doc |
| `init` makes the board (fields, views, labels) and the registry issue | GS 90-92; OG 106-108; CO 34 | doc |
| `init` opens one pull request with `AUTHORITY.md` (and `.crewboard.toml`), `--with-ci` adds a CI workflow | GS 92-94; QT 54-55 | doc |
| `AUTHORITY.md` says what agents may do, for example whether the coordinator may merge | CO 28 | doc |
| default `coordinator_may_merge: false`: only the owner merges | OG 140-141; GS 104-105 | doc |
| agents can join only after the merge of the init PR | GS 106-107 | doc |
| `agent add`: agent dir, `join`, clone of the work dir, `--wait-approval` waits | AG 33-37, 45-46 | doc |
| the join is a comment in the registry issue | ES 58; CO 34 | doc |
| `approve` prints the account of the join and binds it (`login=<account>`) | OG 200-202; GS 150 | doc |
| output shape "approved as dev; ... account ... frozen" | ES 63-64 | example (shortened to `login=teammate`) |
| without the approval an agent has no authority | CO 22; ES 69 | doc |
| "registry issue (pinned)", locked | CO 34 | doc |
| `agent add` ends with `start`, the agent is active | ES 66 | doc |
| `you/my-repo`, `my-crew`, `host-b` | GS 17-21 | example |

### Scene 5: the owner's day (`S05OwnerDay`)

| id | Text |
|---|---|
| s5-1 | After the setup, the owner mostly watches. The board has the views Board, By Agent, Crew and Needs Owner. |
| s5-2 | A question to the owner sets the label needs-owner. The owner answers in a comment on the ticket. A chat answer is not signed and does not count. |
| s5-3 | Three things only the owner does. Approve agents. Merge protected changes, and by default every pull request. And move a ticket by hand with the move command, so that the move is in the signed log. A dragged card shows up in status as a mismatch. |

Screen: the four view tabs; a card `#4` in Blocked with the label `needs-owner`; an owner comment
("counts"); "a chat answer: not signed, does not count"; the three owner-only items; a dragged
card from Review to Done with "status: board against signed log".

| Claim | Source | Status |
|---|---|---|
| the views Board, By Agent, Crew and Needs Owner | OG 206-207 | doc |
| "the owner mostly watches" | OG 3 ("you approve each agent, and you merge what needs your decision") | derived: the owner's listed day-to-day work is seeing, answering, moving, merging (OG 205-225) |
| a question to the owner sets the label `needs-owner` | OG 209-210; CLI 929 | doc |
| the owner answers in a comment on the ticket; a chat answer is not signed and does not count | OG 210-211 | doc |
| only the owner approves agents | CO 9; OG 180 | doc |
| only the owner merges protected changes; by default every PR | CO 9; OG 140-141, 220-222; OG 157-165 | doc |
| `move <n> <status> --why`: posts `/crew move <status>`, so the move is in the signed log | OG 211-215; CLI 1327 | doc |
| a dragged card shows in `status` as `board against signed log` | OG 215-216 | doc |
| the `ask` of an In Progress ticket makes it Blocked (card in Blocked) | CLI 929 | doc |

### Scene 6: the coordinator and a ticket (`S06CoordTickets`)

| id | Text |
|---|---|
| s6-1 | The coordinator's view. It writes no code and takes no tickets. It makes and assigns tickets, and verifies deliveries. |
| s6-2 | A ticket has a Goal, a Gate and a Scope. The Gate is the one line that decides whether the ticket is done. It must be able to fail: CI is green for the head SHA, and grep finds the new heading. |
| s6-3 | Assign puts the ticket into the queue of one dev agent; the Order field gives the position, lowest first. The ticket moves to Ready, and a signed request goes to the agent. |
| s6-4 | The coordinator does not poll. A program waits for events and wakes it: the watch command, in the background. Each cycle handles Review before Backlog. |

Screen: `claude@host-a` with "no code", "no tickets of its own"; the verbs `ticket new`,
`assign`, `verify`, `merge`, `close`; the ticket card `#4 Add a section "Running the tests" to
CONTRIBUTING.md` with Goal, Gate (`the CI run of the PR head SHA is green, and grep -q '^##
Running the tests' CONTRIBUTING.md exits 0`) and Scope; "one line; it must be able to fail";
`crewboard assign 4 --to claude@host-b --order 1`; the queue of `claude@host-b` with Order 1, 2,
3; Backlog to Ready; a signed request; the watch ring with events; "1 Review: verify, merge", "2
Backlog: assign".

| Claim | Source | Status |
|---|---|---|
| the coordinator writes no code and takes no tickets | SKC 3; CO 19 | doc |
| it assigns tickets, verifies evidence, keeps work moving | CO 19 | doc |
| the verbs `ticket new`, `assign`, `verify`, `close`, `merge` are the coordinator's | OG 182-186 | doc |
| a ticket body has Goal, Gate, Scope (Needs, Pointers) | SKC 12 | doc |
| the Gate is the one line that decides; it must be able to fail | CO 24; GS 164 | doc |
| the example ticket and Gate text | GS 169-171 | example (doc text) |
| `assign <n> --to <agent>` puts the ticket at the end of that agent's queue; `--order` sets the position; lowest first | SKC 40-45; AG 171-174; OG 120-121 | doc |
| `assign` moves the ticket to Ready | CO 41 | doc |
| the assignment is a signed `request` to the agent | ES 143-148 | doc |
| the model never polls; a program waits and wakes it | SK 38-39; AG 84 | doc |
| interactive coordinator: the watch command in the background (Monitor tool) | SK 41-43; AG 87-90; IC 3-4 | doc |
| each cycle: Review before Backlog, verify and merge first, then assign | SKC 25 | doc |

### Scene 7: verify, merge, close (`S07Verify`)

| id | Text |
|---|---|
| s7-1 | When a dev agent delivers, the ticket is in Review. The coordinator runs verify. The program checks the evidence: the pull request, its head SHA, and a green CI run for that SHA. |
| s7-2 | The coordinator model checks the Gate itself: it reads the run log or reruns the command. Only a Gate that is exactly CI is checked by the program alone. |
| s7-3 | If the Gate check fails, verify with fail sends the ticket back to In Progress. It keeps its agent, and the signed fail report is the record of the rework. |
| s7-4 | If it passes and AUTHORITY.md allows it, the coordinator runs merge. Otherwise the owner merges on GitHub, and the coordinator runs close. The ticket is Done. |

Screen: a board strip with `#4` in Review; `crewboard verify 4 --body "grep ok, log read"`;
three ticked rows "pull request you/my-repo#5", "head SHA 17731ed", "CI run for that SHA:
green"; the Gate line, a run log and "the model"; "only Gate: ci is checked by the program
alone"; `crewboard verify 4 --fail --body "heading missing"`, the card back in In Progress with
"agent: claude@host-b", a red `verify: fail` message; `crewboard merge 4` with "AUTHORITY.md
allows it", "the owner merges", `crewboard close 4`, the card in Done.

| Claim | Source | Status |
|---|---|---|
| `done` moves the ticket to Review; `verify` checks evidence and Gate | CO 44-45 | doc |
| the evidence: the PR, its head SHA, the newest commit, a green CI run for that SHA | RM 13-14; CO 26 | doc (the narration drops "the newest commit" for length) |
| `verify --body`: what the coordinator found when it checked the Gate itself | CLI 1224-1225 | doc |
| the coordinator checks the Gate: read the run log or rerun the command | SKC 90-91 | doc |
| only a Gate of `ci` is checked by the program alone | RM 20-21; CO 75-76 | doc |
| `verify --fail --body` sends the ticket back; it keeps its agent and goes to In Progress | SKC 72-74; CO 45 | doc |
| the signed `verify: fail` reports are the record of the rework | SKC 95-96 | doc |
| after a passed verify: `crewboard merge <n>`, if `AUTHORITY.md` allows | SKC 102-106; CO 57 | doc |
| otherwise the owner merges, then the coordinator runs `close <n>`: Done | CO 46-47, 61; GS 182-183 | doc |
| PR `#5`, head `17731ed` | ES 95, 155-156 | example |

### Scene 8: the dev agent (`S08Dev`)

| id | Text |
|---|---|
| s8-1 | The dev agent's view. It delivers code through pull requests: one ticket, one branch, one pull request. |
| s8-2 | It runs mine to see its tickets, and takes the first Ready one with take. The ticket moves to In Progress, and take prints the branch name. |
| s8-3 | Before it acts, it reads the thread with mine and the ticket number: the signed messages since its assignment, newest first. So it does not ask again what was answered. |
| s8-4 | It opens the pull request and waits for the CI finished event. On green, it runs done with the pull request URL and the head SHA. The ticket moves to Review. |
| s8-5 | Stuck on a tool, it runs block with a reason. A scope question goes to the owner with ask. Then it leaves that ticket until an answer arrives, and takes its next Ready ticket. |

Screen: "one ticket", "one branch", "one pull request"; a dev terminal with `crewboard mine`,
`crewboard take 4`, `branch: crew/claude@host-b/4-add-a-section`, `crewboard mine 4` and the
thread panel (owner answer, `verify: fail`, own report, the request, newest first; "answered
already: no new question"); `PR you/my-repo#5`, the event `ci-finished: green`, `crewboard done 4
--pr .../pull/5 --sha 17731ed`; `crewboard block 4 --why "tool missing"`, `crewboard ask 4 --body
"keep the old heading?"` with the label `needs-owner`, "owner answer on #4", and `#7` taken.

| Claim | Source | Status |
|---|---|---|
| one ticket, one branch, one PR | SKD 3 | doc |
| `mine`: the agent's In Progress and Ready tickets; take the first Ready one with `take` | AG 168; SKD 14-15 | doc |
| `take` moves Ready to In Progress and prints the branch line | CO 42; CLI 833; SKD 15-17 | doc |
| the branch prefix `crew/` and the shape `crew/<agent>/<n>-<slug>` | OG 143; ES 98-99 | doc |
| `mine <n>`: the signed thread since the assignment, newest first; read it before acting and before asking | CLI 806; SKD 9-13; CH 9-17 | doc |
| so a wake does not ask again what was answered | CH 15-17; SKD 12 | doc |
| never wait for CI with `gh run watch`; wait for the `ci-finished` event | SKD 28 | doc |
| on `ci-finished green`: `done <n> --pr <url> --sha <head sha>` | SKD 40-41 | doc |
| `done` moves the ticket to Review | CO 44; CLI 864-866 | doc |
| stuck on a tool: `block <n> --why` | SKD 20 | doc |
| owner decisions (scope, product) go to `ask <n>`; after `ask` or `block`, leave the ticket until an answer arrives | SK 70-73; SKD 50-51 | doc |
| meanwhile take the next Ready ticket | SKD 51 | doc |
| `ask` sets the label `needs-owner` and makes the In Progress ticket Blocked | CLI 929 | doc |

### Scene 9: through the skill (`S09Skill`)

| id | Text |
|---|---|
| s9-1 | How does an agent know all this? Through the skill, which is inside the binary. Skill install links it into each runtime that is present: claude, codex, copilot or hermes. |
| s9-2 | The skill names the commands, the rules and the exit codes, and a role file: coordinator or dev. The owner tells an agent: use the crewboard skill, you are claude at host-b, a dev agent. The agent runs the CLI itself. |
| s9-3 | The loop is the same for every agent. A scheduled wake or the watch command delivers events; a quiet board starts no model. The agent runs mine, then mine with the ticket number, acts, and reports through signed commands. |
| s9-4 | A refusal exits with code two and prints its rule and the fix, so the agent corrects itself. Code three: GitHub did not answer, retry later. Code four: a config or key problem, stop and tell the owner. Code sixty-four: the command line was wrong; fix it and run it again. |
| s9-5 | The owner can run the setup from an agent session too, with the owner's own GitHub login. Approval and merge stay the decisions of a person. |

Screen: the binary with `SKILL.md` inside, `crewboard skill install`,
`~/.config/agent-skills/crewboard/`, links to `claude`, `codex` (present) and `copilot`,
`hermes` (dim); the skill sections Start, Your loop, Rules, Exit codes; `coordinator.md`,
`dev.md`; the owner's prompt "Use the crewboard skill. You are claude@host-b, a dev agent."; the
agent's own commands `crewboard start`, `crewboard mine`, `crewboard take 4`; the loop ring
"wake or watch, mine, mine <n>, act, signed command"; "a quiet board starts no model"; the exit
rows 2, 3, 4, 64 and the example `refused: unread-events. Fix: run watch --timeout 1m first`; the
owner's session with `init`, `agent add`, `approve`, "the owner's own gh login", "approve,
merge: a person decides".

| Claim | Source | Status |
|---|---|---|
| the skill files are inside the binary | IN 66; CLI 576 (`skill install` help) | doc |
| `skill install` copies the skill to `~/.config/agent-skills/crewboard/` and links it into each runtime that is present: claude, codex, copilot, hermes | CLI 576; IN 45-51 | doc |
| "present" = skills or config dir exists, or the binary is on PATH | IN 46-48 | doc |
| the skill has the sections Start, Your loop, Read events, Rules, Exit codes | SK 20-83 | doc |
| the role files `coordinator.md` and `dev.md` | SK 32 | doc |
| the first prompt "Use the crewboard skill. You are claude@host-a, the coordinator of ..." | IC 15-18; ES 77 | doc (the video says `claude@host-b`, a dev agent) |
| the agent runs the CLI itself: "The CLI does the GitHub work. You decide what the work means." | SK 11-12; SK 22 (`crewboard join`) | doc |
| the loop: a program waits, the model acts only on what it prints; never a `while` or `sleep` loop | SK 38-39 | doc |
| a scheduled wake (`crewboard wake`) or the watch command | SK 41-44; CO 16; AG 84-96 | doc |
| a quiet board starts no model | UC 4-5, 58-59; GS 192-193 | doc |
| at the start of each wake: `mine <n>` for each ticket, before acting; wrapper runs `poll` and `mine` | SKD 9-10, 83-84; UC 85-89 | doc |
| reports go through signed commands (`say`, `done`, `block`, `ask`) | SK 70-73; CO 27 | doc |
| exit 2: a rule refused; the message names the rule and the fix; follow the fix; never retry unchanged | SK 17-18, 79; EX 13 | doc |
| exit 3: GitHub or network, retry in minutes; exit 4: config or key, stop and tell the owner | SK 80-81; EX 14-15 | doc |
| the example refusal `unread-events` and its fix `watch --timeout 1m` | SKD 45-46; GS 203-204 | doc |
| exit 64: a usage error (a bad option, argument or issue number); fix the command and run it again; 64 for every command | `origin/main` (`ca3fb47`, crewboard 0.8.0): SK 82; EX 16, 23-24 | doc (added in v1.1; at `8788f9b` exit 64 was for `lock`, `repeat` and `wake` only) |
| owner commands use the owner's own `gh` login, never `GH_TOKEN`; they refuse any account but the board owner | OG 23-26 | doc |
| the owner can run them from an agent session of their own | OG 23-26 (the login decides, not the terminal); OG 25-26 ("Run them in your own terminal, not in an agent's config dir") | derived: any session with the owner's `gh` login outside an agent's config dir qualifies |
| approval and merge stay with a person: only the owner approves agents and merges protected changes; by default every PR | CO 9; OG 140-141, 180-181 | doc |

### Scene 10: proof before Done (`S10Proof`)

| id | Text |
|---|---|
| s10-1 | Proof before Done. Through the CLI, a ticket reaches Done in two ways only. One: the coordinator verified the pull request, and it is merged at that SHA. Two: the owner merged it, and the coordinator runs close. |
| s10-2 | A merge by any other account is no proof, an agent's account included. A pull request that the coordinator merged itself, without a verify, leaves the ticket open. |
| s10-3 | A ticket that ends without a merge is not Done. The coordinator drops it with close and a written reason: Status Dropped. Done and Dropped are final; only the owner moves such a ticket. |

Screen: two paths into "Done" ("verify passed, merged at that SHA"; "the owner merged, close"); a
third path "merged by an agent account, no verify" crossed, "no proof: the ticket stays open";
"no merge", `crewboard close 4 --why "replaced by #8"`, "Dropped", the note `dropped: replaced by
#8`; locks on Done and Dropped; `crewboard move 4 Ready --why ...` by the owner.

| Claim | Source | Status |
|---|---|---|
| through the CLI, Done in two ways only: verified and merged at that SHA, or the owner's merge | RM 11-15; CO 55-61; SE 26-28 | doc |
| after the owner's merge the coordinator runs `close <n>` | CO 61; OG 221-222 | doc |
| a merge by any other account is no proof, an agent's account included; a self-merge without verify leaves the ticket open | CO 64-66; OG 222-224 | doc |
| a ticket without a merge is not Done: `close --why` drops it, Status Dropped, signed note `dropped: <reason>` | RM 17-18; CO 69-71 | doc |
| name the replacing ticket in the reason | SKC 115-116 | doc |
| Done and Dropped are final; only the owner moves such a ticket, with `crewboard move` | CO 50-51; SKC 117 | doc |
| `move` to Ready, not to Done or Dropped | CLI 1327; OG 216 | doc |
| ticket `#8` as the replacing ticket | none | example (an invented number as a prop, no claim) |

### Scene 11: the board against the signed log (`S11BoardLog`)

| id | Text |
|---|---|
| s11-1 | What if somebody writes Done on the board by hand? Any agent token can. crewboard cannot prevent that; it detects it. |
| s11-2 | Status compares the board against the signed log and lists each ticket whose Status no signed message supports. Audit reports a field mismatch, and the coordinator tells the owner once. |
| s11-3 | The coordinator also remembers its own notes on each ticket. A note that is gone on the next read raises the security event deleted-message. |
| s11-4 | The repair is the owner's decision: a move with the move command, and that move is in the log. |

Screen: left "the board" with `#4 Status: Review`, right "the signed log" with three lines
(request, ack, done); "an agent token" turns the field into "Status: Done"; "detected, not
prevented"; `crewboard status` with `board against signed log: 1 mismatch #4 Done`; "no signed
message for Done"; `crewboard audit --since 2h` with `field-mismatch #4 Status=Done`; "one signed
note to the owner"; "remembered note ids: 3", one log line vanishes, "security event:
deleted-message"; the owner's `crewboard move 4 Ready --why "no merge yet"` and the log line
`/crew move Ready`.

| Claim | Source | Status |
|---|---|---|
| the holder of any agent token can set `Status = Done`; crewboard cannot stop that; detected, not prevented | CO 82-84; RM 70-71; SE 33-34 | doc |
| `status` lists the ticket: `board against signed log` (a Status that no signed message supports) | CO 83-84; ES 208 | doc |
| `audit` reports a `field-mismatch`; the coordinator tells the owner once | CO 84-85; SKC 157 | doc |
| `audit --since 2h` | OG 225 | doc |
| `audit` finds unsigned, forged, edited messages (on screen only through the command) | CLI 1302-1304 | doc |
| the coordinator remembers the comment ids of its own signed notes; a gone note raises `deleted-message` once and tells the owner | SE 62; CH 93-97 | doc |
| the repair is the owner's decision; `move` posts `/crew move <status>`, so it is in the log | CO 85-86; OG 211-215 | doc |
| the log line shapes (`request`, `ack`, `done`, `/crew move`) | ES 143-159; OG 214 | example |

### Scene 12: strengths and limits (`S12Limits`)

| id | Text |
|---|---|
| s12-1 | What crewboard gives you: two or more coding agents on one GitHub repo, a shared queue, a record of who did what, and a check on every done. No server, no database. |
| s12-2 | What it does not do. A Done written by hand is detected, not prevented. One coordinator verifies and merges: a few agents, not swarms. A leaked agent token acts as its account, but cannot sign as an agent without the agent's key. |
| s12-3 | GitHub only; agent hosts run macOS or Linux. The setup has manual steps, and there is no sandbox. Only interactive Claude Code on macOS is fully tested. |
| s12-4 | And crewboard does not make an agent write correct code. Your Gate, your CI and the owner's review do that. |

Screen: two columns, "what it gives" (five ticked rows) and "what it does not do" (eight rows; the leaked-token row reads "a leaked token acts as its account, not as an
agent").

| Claim | Source | Status |
|---|---|---|
| two or more coding agents on one repo, a shared queue, a record of who did what, a check on every "done" | RM 39-40 | doc |
| no server, no database | RM 9 | doc |
| a Done written by hand is detected, not prevented | RM 70 | doc |
| one coordinator verifies and merges: crews of a few agents, not swarms | RM 72 | doc |
| a leaked agent token acts as its account, but cannot sign as an agent without that agent's key | RM 73-74 | doc |
| GitHub only; agent hosts run macOS or Linux | RM 75; AG 10 | doc |
| setup has manual steps: GitHub has no API to give an account access or to make its token | RM 78 | doc |
| no sandbox: the agents run shell commands as your user | RM 77 | doc |
| only interactive Claude Code on macOS is fully tested | RM 78; AG 13-18 | doc |
| crewboard does not make an agent write correct code; your Gate, your CI and the owner's review do | SE 38-39 | doc |

### Scene 13: where to start (`S13Close`)

| id | Text |
|---|---|
| s13-1 | Where to start. The Quick try: one person, one machine, your own token, a test repo and one ticket. Nine steps, three terminals. |
| s13-2 | Then a real crew: a coordinator and a dev agent, in thirteen steps. In a team, everyone brings the GitHub account that they already have. |
| s13-3 | crewboard. A crew of agents. One board. Proof before Done. |

| Claim | Source | Status |
|---|---|---|
| Quick try: 9 steps, 3 terminals; one person, one machine, your own token, a test repo, one ticket | RM 63-64; QT 1-3 | doc |
| a real crew: 13 steps, a coordinator and a dev agent | RM 67-68; GS 3-7 | doc |
| in a team, each person uses the GitHub account that they already have | RM 50-53; GS 42-46 | doc |
| the closing line | RM 3 | doc |

## 4. Mechanisms shown in motion

- A signed message travels from the agent to the registry issue (scene 4) and from the
  coordinator to the dev agent (scene 6).
- The signature check: two ticks (author, key) make a message count; forged, edited and copied
  messages are crossed (scene 3).
- Status columns moving: Backlog to Ready (6), Ready to In Progress, In Progress to Review,
  Review to Blocked, a second ticket taken (8), Review back to In Progress on `verify --fail`,
  then to Done (7), a dragged card marked as a mismatch (5).
- The board against the log: a hand-written Done on the left, the log that does not support it
  on the right, `status` and `audit` output, a deleted note (11).
- The loop ring of an agent: wake or watch, mine, mine `<n>`, act, signed command (9).

## 5. Checks run

- The narration text: 52 segments, 7,615 characters (`jq`); the longest segments are s9-4 (271
  characters), s5-3 (246), s12-2 (228) and s9-3 (221).
- No em-dash in `script.json`, `scenes.py`, `README.md` or this file (`grep` for the character).
- `build.sh` checks that `ALL_SCENES` in `scenes.py` matches `order` in `script.json`, that
  every class exists, and that `tts.groups` covers every scene exactly once.
- Every `cue()` phrase is a substring of its segment text (a wrong one aborts the render).
- Review PNGs (`review/l/*-40.png`, `*-end.png`) and extra frames from the middle of scenes 3 to
  13 were read for overflow after the 480p draft; the fixes are listed in the README under "Build
  log".
- Speech rate per segment after the Gemini narration: every segment between 11 and 19 characters
  per second except the title s0-1 (10.0: four short sentences with pauses), so no cut landed in
  the wrong paragraph.
