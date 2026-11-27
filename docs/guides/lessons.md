# From a repeated fault to a rule

Work that comes back from a verify costs a round: hours of an agent and minutes of the
coordinator. One fault that comes back three times is a fault of the line, not of an agent.
This page says how the owner finds such a fault and makes a rule of it. crewboard counts; it
writes no rule itself.

## 1. Give each review fail a tag

The coordinator starts the note of a failed verify with one tag in brackets:

```sh
crewboard verify 12 --fail --body "[formatter] the formatter was not run on two files"
```

A tag is one word of lower-case letters, digits and `-` (at most 24). It names the kind of the
fault, not the ticket: `formatter`, `scope`, `evidence`, `gate-part`, `stale-branch`. Keep the
list short and stable; write it into the free text of `AUTHORITY.md`, so every coordinator
uses the same words. A check of the tool that fails gets its tag by itself: `check-<name>`.

## 2. Read the section "Repeated faults"

```sh
crewboard retro --since 14d
```

The section lists each tag with three or more review fails that are rework, with the tickets.
It also shows the tags below three and the number of review fails without a tag. A high number
without a tag means that the coordinator does not tag yet.

## 3. Choose the cheapest rule that binds

Take the first form that fits. A rule that a tool enforces costs nothing per ticket; a
sentence in a prompt costs attention in every wake and still fails sometimes.

| The fault | The rule | Where |
|---|---|---|
| A command was not run (formatter, linter, a test target) | A check that the tool runs | `checks` in `AUTHORITY.md`, and the Gate names it ([Checks that the tool runs](../design/spec/ci.md#checks-that-the-tool-runs)) |
| A file was changed that only you should change | A protected path | `protected_paths` in `AUTHORITY.md` |
| CI did not cover the change | A CI job | the workflow of the repo (the owner route) |
| The ticket was too large or unclear | A rule for the coordinator | the free text of `AUTHORITY.md` |
| A fact about the repo that agents do not know | One line for the agents | the agent instructions of the repo |

## 4. Make it a PR that you merge

A rule changes what agents may do, so it goes the way of every such change: a PR on
`AUTHORITY.md` or on a protected file, merged by you. Write three things into the PR:

- the tag and the tickets from `retro` (the evidence);
- the rule, in one sentence;
- a date to look again.

An agent may draft the text. An agent does not merge it, and the agent whose work failed does
not write its own rule.

## 5. Look again

Run `crewboard retro` for the time after the merge. The tag should fall under three. If it
does not, the rule does not bind: make it stricter or remove it. A rule that no fault needs any
more is removed too: each rule costs attention.

## Read the section "Outcome" too

A fault that no verify found shows up after the merge. The section "Outcome" of `retro` counts
the merged tickets that came back and the merged tickets that a commit of the default branch
reverts. Read the tickets of both lists: each one is a fault that the Gate did not catch, and
a candidate for a check. The owner numbers say how often you had to step in and how long a
question waited for you; a long wait is a cost of the line, not of an agent.

## What this does not do

- It does not classify a fail without a tag. A model would have to read the note, and its
  answer would be a guess.
- It does not prove that a rule caused a change. One repo and a few weeks give small numbers.
- It does not write, propose or merge a rule.
