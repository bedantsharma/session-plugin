# Releasing bridge-screen

## One-time setup (owner)

1. Create the public repo `bedantsharma/homebrew-tap` (empty is fine).
2. Create a GitHub token that can push to that repo and save it in this repo's Actions secrets as `HOMEBREW_TAP_TOKEN`.
3. Create an npm access token (publish rights) and save it as the Actions secret `NPM_TOKEN`.
4. On pypi.org, add a "pending trusted publisher" for project `bridge-screen`: owner `bedantsharma`, repo `session-plugin`, workflow `pypi.yml`, environment `pypi`. Create the `pypi` environment under this repo's Settings → Environments.
5. On crates.io, make sure the crate names `bridge-screen-core` and `bridge-screen` are free
   (search for them; the first publish claims them), create an API token with the
   `publish-new` and `publish-update` scopes, and save it as the Actions secret
   `CARGO_REGISTRY_TOKEN`.

## Cutting a release

1. Bump `version` in the root `Cargo.toml` (`[workspace.package]`) and commit.
2. `dist plan` to check the release.
3. `git tag v<version> && git push origin v<version>`.
4. The `Release` workflow builds binaries, the GitHub Release, installers, the Homebrew formula and npm packages. The `PyPI` workflow uploads wheels. The `crates.io` workflow publishes `bridge-screen-core`, then `bridge-screen` (it skips prerelease tags such as `v0.1.0-rc.1`, because a crates.io version can never be taken back).

## crates.io

- Both crates are published, in this order: `bridge-screen-core`, then `bridge-screen` (which depends on it).
  The binary's dependency on the core names both the path and the version
  (`{ version = "<workspace version>", path = "../bridge-screen-core" }`). Bump it together with the
  workspace version; a test (`plugin_package_is_consistent`) fails if they differ.
- Check by hand without publishing: `cargo package --workspace` (packages and builds both crates; it
  resolves the core from a temporary local registry, so it works before the core is on crates.io).
  `cargo publish -p bridge-screen --dry-run` alone fails until the core is on the index; that is expected.
- `cargo install bridge-screen` builds without the dashboard: `bridge-screen ui` then serves a page that
  says how to build it. Say so in the release notes. The binaries on the GitHub Release, brew, npm and
  PyPI embed it.

## What users run

- `brew install bedantsharma/tap/bridge-screen`
- `pipx install bridge-screen` or `uv tool install bridge-screen`
- `npx bridge-screen`
- `curl -LsSf https://github.com/bedantsharma/session-plugin/releases/latest/download/bridge-screen-installer.sh | sh`

## The embedded web app

The binary embeds the dashboard (`web/dist`) and the desktop card (`web/dist-card`) at
build time. Every release build runs `npm --prefix web ci && npm --prefix web run build`
first: the `Release` workflow through dist's `github-build-setup` hook
(`.github/build-setup.yml`, set in `dist-workspace.toml`), the `PyPI` workflow and the CI
Rust jobs as a step of their own. Without a web build (e.g. `cargo install` from
crates.io) the binary still builds and `bridge-screen ui` serves a page saying how to
build the dashboard; the JSON API works either way. After changing
`.github/build-setup.yml`, run `dist generate --mode ci`.
