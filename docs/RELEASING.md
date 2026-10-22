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

## Before the first tag (checklist)

Do these in order; each says what "done" looks like.

1. **The five one-time setup items above are done**: tap repo and `HOMEBREW_TAP_TOKEN`, `NPM_TOKEN`,
   the PyPI pending trusted publisher with the `pypi` environment, and `CARGO_REGISTRY_TOKEN`
   (the crate names checked as free). Done when the Actions secrets page lists the three tokens and the
   `pypi` environment exists.
2. **`dist plan` reviewed.** It must list `bridge-screen-<arch>-<os>.tar.xz` archives (with `.sha256`
   files) for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and
   `aarch64-unknown-linux-gnu`: the plugin launcher downloads exactly those names. It also lists the
   Windows zip, the shell and PowerShell installers, the Homebrew formula and the npm package.
3. **`claude plugin validate ./plugin` passes**, and `plugin/.claude-plugin/plugin.json`'s `version`
   equals the workspace version.
4. **Rehearse with a prerelease tag first**: bump the workspace version to `0.1.0-rc.1`, tag
   `v0.1.0-rc.1`. dist marks it a prerelease and skips the Homebrew and npm publish; the `crates.io`
   workflow skips it too. Check the GitHub Release page, the installers and the PyPI upload. Done when
   the archives download and `bridge-screen --version` runs from one of them.
5. **Then the real tag**: set the version to `0.1.0`, commit, tag `v0.1.0`, push the tag.

Windows: the program itself is released for Windows (zip, PowerShell installer, wheel, npm). The
plugin's own launcher is not: on Windows it tells the user to install the program and put it on
their PATH, and from a hook it exits quietly. **Owner decision recorded, not made here:** real
Windows plugin support needs a Windows machine to test on, so it is left as a follow-up.

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
