# Packaging reHydrate for distribution

`cargo run -p rehydrate-app --release` runs the app from source. For
distributable installers the project uses Tauri's bundler — locally
via `cargo tauri build`, or in CI via the `Release` workflow which
attaches built bundles to a draft GitHub Release.

reHydrate v1.0 ships ARM-only macOS. Intel Macs, Windows, and Linux
are not currently shipped as binaries; the code is platform-agnostic
and builds locally on each, but the release pipeline produces a
single `aarch64-apple-darwin` `.dmg`.

## Cutting a release

```sh
git tag v1.1.0
git push origin v1.1.0
```

The `.github/workflows/release.yml` workflow runs on a GitHub-hosted
`macos-14` (Apple Silicon) runner. For each tag it:

1. Builds the UI bundle (`npm ci && npm run build`).
2. Runs `cargo tauri build --target aarch64-apple-darwin --bundles
   app,dmg -- --no-default-features`, producing
   `reHydrate_<version>_aarch64.dmg` and an `.app.tar.gz` archive.
3. Uploads both to a *draft* GitHub Release at the tag.
4. Hashes the bundles with `shasum -a 256` and uploads a
   `SHA256SUMS` file alongside them.

Review the artefacts (and the workflow logs that emit the hashes),
then click **Publish release** in the GitHub UI to make the build
public.

A manual `workflow_dispatch` run produces the same artefact as an
ordinary workflow artefact (no Release entry), useful for testing
the build itself.

## Prerequisites for local builds

```sh
cargo install tauri-cli --version "^2.0.0"
```

The UI bundle has to be built before `cargo tauri build` runs (the
`frontendDist` in `crates/rehydrate-app/tauri.conf.json` points at
`ui/dist/`, which `tauri build` reads but doesn't generate):

```sh
(cd ui && npm ci && npm run build)
cargo tauri build -- --no-default-features
```

`./build.sh` runs the same UI build + `cargo run` for quick local
iteration; it does *not* produce a bundle. `cargo tauri dev` runs the
dev workflow with hot reload (Vite serves the UI directly, no static
bundle needed).

Why no `beforeBuildCommand` in `tauri.conf.json`? In a Cargo
workspace with `projectPath: crates/rehydrate-app`, `tauri-action`'s
cwd handling silently desyncs relative paths in the build hook —
`../../ui` resolves one directory too high on CI runners and `tauri
build` then fails on every platform with `ENOENT package.json`.
Pre-building the UI explicitly (in the workflow, in `build.sh`, or
by hand) is more robust.

## `--no-default-features` is mandatory

The `devtools` feature on `rehydrate-app` is default-on so daily
`cargo run` / `cargo tauri dev` works without flags. The release
workflow must build with `--no-default-features` so shipped binaries
don't expose the inspector to end users — the renderer can call any
registered Tauri command, so an open DevTools is a security boundary
skip. Both the workflow and the local-build snippets above include
this flag.

## What's left

### Icons

Done — the icon set is generated from `assets/logo.png` via
`tauri icon`:

- `icons/32x32.png`, `128x128.png`, `128x128@2x.png` (generic)
- `icons/icon.icns` (macOS)
- `icons/icon.ico` (kept for future Windows builds)

Regenerate with `npx @tauri-apps/cli icon ../../assets/logo.png` from
`crates/rehydrate-app/`. `bundle.active` is `true` in
`crates/rehydrate-app/tauri.conf.json`.

### macOS signing + notarization

`tauri.conf.json` ships with `signingIdentity: "-"` — the magic
value that tells Tauri (and the underlying `codesign`) to **ad-hoc
sign** the whole bundle. Required even without a Developer ID,
because:

- On Apple Silicon, the linker stamps an ad-hoc signature on the
  Mach-O executable that claims the bundle has sealed resources.
- Without a follow-up `codesign --force --deep --sign -` on the
  bundle, those resources aren't actually sealed (`codesign -dv`
  reports `Sealed Resources=none`).
- The kernel rejects the signature mismatch at load time, and
  macOS shows the misleading **"App is damaged and can't be
  opened"** error — even with right-click → Open, even with
  `xattr -d com.apple.quarantine`. The bundle is unlaunchable.

`signingIdentity: "-"` produces a properly self-consistent ad-hoc
bundle that passes the kernel check. Gatekeeper still warns on
first launch (right-click → Open clears it) because there's no
Developer ID, but the app actually launches.

For a real Developer ID build, replace with:

- `signingIdentity`: the common name of your Developer ID
  Application certificate, e.g. `"Developer ID Application: Your
  Name (TEAMID)"`.
- `providerShortName`: your App Store Connect provider short name
  (only needed if your Apple ID is on multiple teams).
- `entitlements`: path to a `.plist` if you need the hardened
  runtime with specific exceptions. The app does not currently
  need any.

Notarization requires environment variables at `tauri build` time:

```sh
export APPLE_ID='you@example.com'
export APPLE_PASSWORD='app-specific-password'
export APPLE_TEAM_ID='ABCDE12345'
cargo tauri build -- --no-default-features
```

The app makes no outbound network calls beyond `10.11.99.1` (the USB
endpoint), the user-configured Ollama host, and the user-configured
publishing host — see `SECURITY.md`. The default sandbox profile is
fine.

### Verifying a build is clean

After `cargo tauri build`, the bundle lands in
`target/aarch64-apple-darwin/release/bundle/dmg/`. Smoke checks
before shipping:

1. Install the bundle on a clean Mac (or a fresh user account).
2. Launch with the device unplugged. Welcome screen renders, status
   pill shows "No tablet". App is fully usable for browsing an existing
   library in read-only mode.
3. Plug the device in. Pill turns green. Connect → password dialog →
   sync. Library list populates.
4. Disconnect cable mid-sync. App reports the partial sync, library
   stays internally consistent (`Verify` reports zero issues).
5. Run the privacy-invariant test (`cargo test --test no_egress`) to
   confirm the bundle attempts no external network destinations
   besides `10.11.99.1`, the configured Ollama host, and the
   configured publishing host.

### Auto-update

Not configured. The design says auto-update should be opt-in. When you
add it, do not enable check-on-launch by default — keep the
no-telemetry-out-of-the-box invariant.
