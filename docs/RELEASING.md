# Releasing bridge-screen

## One-time setup (owner)

1. Create the public repo `bedantsharma/homebrew-tap` (empty is fine).
2. Create a GitHub token that can push to that repo and save it in this repo's Actions secrets as `HOMEBREW_TAP_TOKEN`.
3. Create an npm access token (publish rights) and save it as the Actions secret `NPM_TOKEN`.
4. On pypi.org, add a "pending trusted publisher" for project `bridge-screen`: owner `bedantsharma`, repo `session-plugin`, workflow `pypi.yml`, environment `pypi`. Create the `pypi` environment under this repo's Settings → Environments.

## Cutting a release

1. Bump `version` in the root `Cargo.toml` (`[workspace.package]`) and commit.
2. `dist plan` to check the release.
3. `git tag v<version> && git push origin v<version>`.
4. The `Release` workflow builds binaries, the GitHub Release, installers, the Homebrew formula and npm packages. The `PyPI` workflow uploads wheels.

## What users run

- `brew install bedantsharma/tap/bridge-screen`
- `pipx install bridge-screen` or `uv tool install bridge-screen`
- `npx bridge-screen`
- `curl -LsSf https://github.com/bedantsharma/session-plugin/releases/latest/download/bridge-screen-installer.sh | sh`
