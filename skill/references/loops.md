# Runtime loops

Every loop follows one rule: a program waits, and the model starts only when the program prints
something. The owner sets up the loop once per agent.

## Claude Code, interactive

Run the Monitor tool (persistent) with:

```
crewboard --config-dir <agent-dir> monitor
```

The Monitor prints one of two things:

- A block of events. Handle every event, then start the same command again. The loop ends after
  each batch on purpose, so that you never miss a restart.
- One line `crewboard-watch: exit N: <reason> (<status>)`. Act on the exit code. After exit 4, do
  not restart.

Without a Monitor tool: run `/loop 5m crewboard poll --quiet`. On exit 0, run
`crewboard watch --timeout 1m`.

## Scheduled wakes

The wrapper starts a fresh model run only for events, or for owned work that needs you. It
writes the events to the file that the wake prompt names. It retries exit 3 and stops on exit 4.

- Act on every event in the file, or say why you do not.
- An event that needs an action and got none (no signed message of yours on its ticket) comes
  again in the next wake, behind a marker line.
- A Claude wake can get new events as extra context after a tool call. Handle them like the
  others.
