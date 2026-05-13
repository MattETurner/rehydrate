# Contributing to reHydrate

Thanks for considering a contribution! reHydrate is a small, privacy-focused
desktop app for managing a reMarkable 2 over USB, and it stays useful because
people like you file bugs, suggest features, and send patches.

This document covers the practicalities. The [Code of Conduct](CODE_OF_CONDUCT.md)
covers how we work together.

## Where to start

- **Found a bug?** Open a [bug report](https://github.com/dm807cam/rehydrate/issues/new?template=bug_report.yml).
  Logs from `Help → Open log directory` are gold — please attach them.
- **Have an idea?** Open a [feature request](https://github.com/dm807cam/rehydrate/issues/new?template=feature_request.yml).
  Describe the workflow you'd like, not just the feature.
- **Found a security issue?** Don't open a public issue. See
  [SECURITY.md](SECURITY.md) for private disclosure.
- **Want to pick up an issue?** Comment on the issue first so we don't
  duplicate effort. Anything labelled `good first issue` is fair game
  for a first PR.

## Development setup

Prerequisites:

- **Rust** stable, ≥ 1.82 (`rustup install stable`)
- **Node.js** 20.19+ or 22.12+
- **npm** (ships with Node)
- **Tauri CLI** for bundle builds: `cargo install tauri-cli --version "^2.0.0"`

A reMarkable 2 plugged in over USB is needed only for the device-dependent
smoke tests (and, of course, for actually using the app). Everything else
works against the fake device in `crates/rehydrate-device/src/fake.rs`.

Clone, install UI deps, and you're ready:

```sh
git clone https://github.com/dm807cam/rehydrate
cd rehydrate
(cd ui && npm install)
```

## Everyday workflows

**Hot-reload dev loop** (Vite serves the UI; Rust rebuilds on change):

```sh
cargo tauri dev
```

**Build a release bundle** (`.dmg` + `.app` on Apple Silicon):

```sh
./build.sh           # bundle only
./build.sh --open    # ... and reveal in Finder
./build.sh --install # ... and copy to /Applications
./build.sh --dev     # quick: cargo run -p rehydrate-app --release
```

**Run all checks locally** (matches CI):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
(cd ui && npx tsc --noEmit && npx eslint . && npm run build)
```

**Device-dependent smoke tests** (need a tablet plugged in):

```sh
# Tablet password: Settings → Help → Copyrights and licenses
MARGINALIA_RM_PASSWORD='...' \
    cargo test -p rehydrate-device --features ssh \
    --test ssh_smoke -- --ignored --nocapture

MARGINALIA_RM_PASSWORD='...' \
    cargo test -p rehydrate-sync \
    --test full_pull_smoke -- --ignored --nocapture
```

## Code style

- **Rust**: `cargo fmt` + `cargo clippy -- -D warnings`. Both run in CI.
- **TypeScript / React**: `tsc --noEmit` + `eslint`. Both run in CI.
- **Comments**: write *why*, not *what*. The code already says what it does.
- **Error handling**: `Result<T, E>` with `thiserror` in libraries; map to
  `String` at the Tauri IPC boundary. No `unwrap()` in non-test paths.
- **No emojis** in commit messages, code comments, or docs unless
  there's a concrete UI reason.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short summary>

<optional body — wrap at 72>
```

Types we use: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`,
`build`, `ci`, `chore`, `style`. Scopes are usually a crate name or `ui`.

Examples from the history:

```
feat(ocr): blank-page filtering before model call
fix(ui): unblock the three v1.0.0 UX dead-ends
build(deps): bump russh from 0.59 to 0.60
```

One logical change per commit is the goal; rebasing to clean up before
opening a PR is welcome.

## Pull requests

- **Small is good.** A 200-line PR gets reviewed; a 2000-line PR gets
  postponed. Split if you can.
- **Tests for behavioural changes.** New code path → new test. Bug fix
  → regression test that fails on `main` and passes with your change.
- **Update `CHANGELOG.md`.** Add a line under the `[Unreleased]`
  section — `Added`, `Changed`, `Fixed`, `Security`, or `Removed`.
- **CI must be green.** fmt, clippy, tests, UI typecheck + lint, and
  the `no_egress` privacy invariant all run on every PR.

The PR template covers the checklist; fill it in honestly.

## Licence

reHydrate is dual-licensed under [MIT](LICENSE-MIT) **or**
[Apache-2.0](LICENSE-APACHE), at your option. By submitting a PR you
agree your contribution is licensed under the same terms, with no
additional restrictions. We don't ask you to sign a CLA.

## Help, questions, anything else

- General questions → open a GitHub Discussion (once enabled) or a
  question-tagged issue.
- Private channel for sensitive things → see SECURITY.md.

Thank you. Patches that make this app more useful for everyone are
the entire point.
