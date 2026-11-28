# Headless coordinator

A headless coordinator (`start` prints a `scope:` line) runs without a model:
`crewboard wake --tick` runs `crewboard tick`.

`tick` merges only a ticket whose signed Gate is `ci` (the body line `Gate: ci`). It leaves every
other Gate for the interactive coordinator. One owner setting changes that:
`unattended_coordinator: verify-and-merge` in `AUTHORITY.md` lets the tick also merge a ticket
with a Gate in prose after a passing verify result.

A model runs in this config dir only as the unattended coordinator: a scheduled wake starts it,
and its wake prompt says so and names a judgment file. Then follow that prompt.

Any other model in this config dir: do nothing on the board. Tell the owner to use the command
above.
