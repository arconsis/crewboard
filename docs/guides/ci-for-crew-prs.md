# CI for crew pull requests

Read this if the main repo has no CI workflow yet, or if you write one by hand. The page belongs
to the [Owner guide](owner.md).

`done` needs a CI run for the head of each crew PR. Agents never edit workflows, so you add one.
`crewboard init --with-ci` puts one into the init pull request when the main repo has no
workflow. It needs the `workflow` scope on your `gh` login (`init` names the `gh auth refresh`
line when it is missing). It picks the test command from the root files of the repo
(`package.json`, `pyproject.toml`, `requirements.txt`, `go.mod`, `pom.xml`, `gradlew`,
`Cargo.toml`; the first file in this order wins). A repo that none of them fits gets a step that
fails until you edit it, so that a green check always means a real test run. Check the test step
before you merge.

To add the file by hand, use this as `.github/workflows/ci.yml`. Replace `<default-branch>` and
`<your test command>` (for example `make test` or `npm test`):

```yaml
name: ci
on:
  push:
    branches: [<default-branch>]
    paths-ignore: ['docs/**', '.crewboard.toml', 'AUTHORITY.md']
  pull_request:          # every crew PR (branch crew/...) gets a run for its head
    branches: [<default-branch>]
permissions:
  contents: read
jobs:
  test:
    runs-on: ubuntu-latest   # a GitHub-hosted runner; for a self-hosted runner use its label
    steps:
      - uses: actions/checkout@v4
      - run: <your test command>
```

Keep the `paths-ignore` of the `pull_request` trigger empty: a crew PR that changes only docs
needs a run too. A workflow change is always yours ([owner route](extensions.md#the-owner-route)).
The rules and the warnings of `init` and `start` are in
[CI and crewboard-only commits](../design/spec/ci.md#ci-and-crewboard-only-commits). For CI jobs
on a host that agents share, see [The host lock](ci-and-host-lock.md).
