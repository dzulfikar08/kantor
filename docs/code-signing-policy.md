# Code signing policy

**Windows:** Kantor has applied to the SignPath Foundation for free code signing. Once the
application is accepted, Windows release files (`Kantor-Setup-*.exe`, `kantor.exe`,
`kantor.com` in the zips) will carry this notice:

> Free code signing provided by [SignPath.io](https://signpath.io), certificate by
> [SignPath Foundation](https://signpath.org).

Until then the Windows builds are unsigned (see [windows-signing.md](windows-signing.md)).

**macOS:** `Kantor.dmg` and `Kantor.app` are signed with the maintainer's Apple Developer ID
(team `F6Q6UFUK69`) and notarised by Apple.

**Updates on every platform** are checked against an Ed25519-signed manifest before they are
installed (see [README, Updates](../README.md#updates)).

## What gets signed

Only files built by the GitHub Actions workflow `.github/workflows/release.yml` from a version
tag (`vX.Y.Z`) of this public repository, on GitHub-hosted runners. Nothing built on a personal
computer is ever signed. The release workflow runs in the `release` environment, which only
`main` and `v*` tags can use.

## Team roles

| Role | Who |
|---|---|
| Committers and reviewers | [Dzulfikar Ats Tsauri](https://github.com/dzulfikar08) |
| Approvers (start a signed release) | [Dzulfikar Ats Tsauri](https://github.com/dzulfikar08) |

Contributions from others are reviewed by a committer before they are merged. The maintainer's
GitHub account uses two-factor authentication.

## Privacy

See [PRIVACY.md](../PRIVACY.md). In short: Kantor doesn't transfer any information to other
networked systems unless you ask it to, except a once-a-day update check (which you can turn
off) that sends only its version, OS and CPU type.
