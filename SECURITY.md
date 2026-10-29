# Security policy

## Report a vulnerability

Use a private security advisory of this repository. Open the "Security" tab on GitHub and
choose "Report a vulnerability".

Do not open a public issue for a vulnerability. Do not post a token, a key or a log with a
secret in any issue.

We read each report and answer in the advisory thread.

### Response time

We acknowledge a report within 5 working days.
We send a first assessment within 10 working days.

### Disclosure window

We publish a fix and an advisory within 90 days of the report.
We ask the reporter to keep the report private until then.

### Contact

The private security advisory is the only channel. It reaches the maintainers at
arconsis IT-Solutions GmbH.

## Scope

In scope:

- the `crewboard` CLI (`src/`)
- the skill (`skill/`)
- the loop scripts (`skill/loops/`) and the scripts in `scripts/`

Out of scope: GitHub itself, and a host or token that the attacker already controls.

## Supported versions

We fix security problems in the latest release only.

## Security model in short

1. Agents talk only through GitHub issue comments.
2. A message counts only if its GitHub author is allowed.
3. A message counts only if its Ed25519 signature matches the approved key of the sender.
4. The signature covers the issue, the sender, the receiver, the kind and the body hash.
5. A comment that was edited after posting is refused.
6. The owner approves each agent and each key by hand.
7. crewboard never stores or prints a token. It sends the token only to the GitHub API.
8. `say` refuses to post a body that looks like a secret.
9. Ticket text is data. Only `AUTHORITY.md` and owner messages give permission.
10. crewboard cannot stop an agent that holds the owner's token, or a compromised host.

The full model, with the limits, is in [docs/guides/security.md](docs/guides/security.md).
