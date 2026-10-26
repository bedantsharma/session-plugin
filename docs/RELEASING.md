# Releasing bridge-screen

## One-time setup (owner)

1. Create the public repo `bedantsharma/homebrew-tap` (with a README: the release job can't check out an empty repo).
2. Create a GitHub token that can push to that repo and save it in this repo's Actions secrets as `HOMEBREW_TAP_TOKEN`.
3. Create an npm access token (publish rights) and save it as the Actions secret `NPM_TOKEN`. npm may still ask a
   one-time code on the first publish of a new package; see "npm" below.
4. On pypi.org, add a "pending trusted publisher" for project `bridge-screen`: owner `bedantsharma`, repo `session-plugin`, workflow `pypi.yml`, environment `pypi`. Create the `pypi` environment under this repo's Settings → Environments.

There is no crates.io route (decision #38): don't add a `CARGO_REGISTRY_TOKEN`.

## Before a tag (checklist)

1. **The one-time setup items above are done.** Done when the Actions secrets page lists the tokens and the
   `pypi` environment exists.
2. **`dist plan --tag v<version>` reviewed.** It must list `bridge-screen-<arch>-<os>.tar.xz` archives (with `.sha256`
   files) for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and
   `aarch64-unknown-linux-gnu`: the plugin launcher downloads exactly those names. It also lists the
   Windows zip, the shell and PowerShell installers, the Homebrew formula and the npm package.
   `dist plan` does not copy files; to prove the archives really build, run
   `dist build --artifacts=local --target=<your target> --tag=v<version>` (the v0.1.2 and v0.1.3 releases
   failed at exactly that copy step, which the plan check cannot see).
3. **`claude plugin validate ./plugin` passes**, and `plugin/.claude-plugin/plugin.json`'s `version`
   equals the workspace version (a test checks it, and the core dependency's version too).
4. **Rehearse with a prerelease tag when the release changes** (a new route, new release settings): bump the
   workspace version, the plugin manifest version and the core dependency's `version` in
   `crates/bridge-screen/Cargo.toml` together to `<version>-rc.1`, tag `v<version>-rc.1`. dist marks it a
   prerelease and skips the Homebrew and npm publish; PyPI still uploads it. Done when the archives download
   and `bridge-screen --version` runs from one of them.
5. **Then the real tag**: set all three versions to `<version>`, commit through a pull request, tag the merge
   commit, push the tag.

**The tag must come after the merge, and only once the build is known to work:** `main` is what the plugin
installs from, so a version on `main` with no matching GitHub release breaks fresh plugin installs.

Windows: the program itself is released for Windows (zip, PowerShell installer, wheel, npm). The
plugin's own launcher is not: on Windows it tells the user to install the program and put it on
their PATH, and from a hook it exits quietly. **Owner decision recorded, not made here:** real
Windows plugin support needs a Windows machine to test on, so it is left as a follow-up.

## Cutting a release

1. Bump `version` in the root `Cargo.toml` (`[workspace.package]`), the core dependency's version in
   `crates/bridge-screen/Cargo.toml`, `plugin/.claude-plugin/plugin.json` and the lock file; commit.
2. `dist plan` (and the local `dist build` above) to check the release.
3. `git tag v<version> <merge commit> && git push origin v<version>`.
4. The `Release` workflow builds binaries, the GitHub Release, installers, the Homebrew formula and npm packages. The `PyPI` workflow uploads wheels.

## Licence field

Keep the crates' licence as a plain `license = "PolyForm-Strict-1.0.0"` string. Do not switch to
`license-file`: dist mis-reads an inherited `license-file` as `../../LICENSE.md` from the repo root and
every program build fails. A test (`crates_name_their_licence_by_string`) guards it. The string is not an
SPDX name, which only matters to crates.io, and bridge-screen doesn't publish there.

## npm

The first publish of a new package asks npm for a one-time code even with a "bypass two-factor" token. If the
`publish-npm` job fails with `EOTP`, publish the first version by hand from the release's
`bridge-screen-npm-package.tar.gz` (`npm publish ./bridge-screen-npm-package.tar.gz --access public`, with the
code from an authenticator app added to the npm account). Later versions can use the token.

## What users run

- `brew install bedantsharma/tap/bridge-screen`
- `pipx install bridge-screen` or `uv tool install bridge-screen`
- `npx bridge-screen` (once the package exists)
- `curl -LsSf https://github.com/bedantsharma/session-plugin/releases/latest/download/bridge-screen-installer.sh | sh`

## The embedded web app

The binary embeds the dashboard (`web/dist`) and the desktop card (`web/dist-card`) at
build time. Every release build runs `npm --prefix web ci && npm --prefix web run build`
first: the `Release` workflow through dist's `github-build-setup` hook
(`.github/build-setup.yml`, set in `dist-workspace.toml`), the `PyPI` workflow and the CI
Rust jobs as a step of their own. Without a web build (e.g. a plain `cargo build` of a checkout) the
binary still builds and `bridge-screen ui` serves a page saying how to build the dashboard; the JSON API
works either way. After changing `.github/build-setup.yml`, run `dist generate --mode ci`.
