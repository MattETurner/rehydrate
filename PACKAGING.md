# Packaging reHydrate for distribution

`cargo run -p rehydrate-app --release` runs the app from source. For
distributable installers the project uses Tauri's bundler — locally
via `cargo tauri build`, or in CI via the `Release` workflow which
attaches signed-on-the-runner bundles to a draft GitHub Release.

## Cutting a release

```sh
git tag v0.1.0
git push origin v0.1.0
```

The `.github/workflows/release.yml` workflow fans out to four
runners — macOS Intel, macOS Apple Silicon, Linux (Ubuntu 22.04), and
Windows — and uploads the produced `.dmg`, `.AppImage`, `.deb`, and
`.msi` artefacts to a draft release at the tag. Review the artefacts,
then click **Publish release** in the GitHub UI to make the build
public.

A manual `workflow_dispatch` run produces the same artefacts as
ordinary workflow artefacts (no Release entry), useful for testing
the build itself.

## Prerequisites for local builds

```sh
cargo install tauri-cli --version "^2.0.0"
```

The UI bundle has to be built before `cargo tauri build` runs (the
`frontendDist` in `crates/rehydrate-app/tauri.conf.json` points at
`ui/dist/`, which `tauri build` reads but doesn't generate):

```sh
(cd ui && npm install && npm run build)
cargo tauri build
```

`./build.sh` does this end-to-end. `cargo tauri dev` runs the dev
workflow with hot reload (Vite serves the UI directly, no static
bundle needed).

Why no `beforeBuildCommand` in `tauri.conf.json`? In a Cargo
workspace with `projectPath: crates/rehydrate-app`, `tauri-action`'s
cwd handling silently desyncs relative paths in the build hook —
`../../ui` resolves one directory too high on CI runners and `tauri
build` then fails on every platform with `ENOENT package.json`.
Pre-building the UI explicitly (in the workflow, in `build.sh`, or
by hand) is more robust.

## What's left

### 1. Icons

Done — the icon set is generated from `logo.png` via `tauri icon`:

- `icons/32x32.png`, `128x128.png`, `128x128@2x.png` (Linux/Generic)
- `icons/icon.icns` (macOS)
- `icons/icon.ico` (Windows)

Regenerate with `npx @tauri-apps/cli icon logo.png` from
`crates/rehydrate-app/`. `bundle.active` is `true` in
`crates/rehydrate-app/tauri.conf.json`.

### 2. macOS signing + notarization

In `tauri.conf.json` under `bundle.macOS`:

- `signingIdentity`: the common name of your Developer ID Application
  certificate, e.g. `"Developer ID Application: Your Name (TEAMID)"`.
- `providerShortName`: your App Store Connect provider short name (only
  needed if your Apple ID is on multiple teams).
- `entitlements`: path to a `.plist` if you need the hardened runtime
  with specific exceptions. The app does not currently need any.

Notarization requires environment variables at `tauri build` time:

```sh
export APPLE_ID='you@example.com'
export APPLE_PASSWORD='app-specific-password'
export APPLE_TEAM_ID='ABCDE12345'
cargo tauri build
```

The app makes no outbound network calls beyond `10.11.99.1` (the USB
endpoint), so the default sandbox profile is fine.

### 3. Windows signing

In `tauri.conf.json` under `bundle.windows`:

- `certificateThumbprint`: SHA1 thumbprint of the code-signing
  certificate installed in the Windows certificate store.
- `timestampUrl`: a public RFC 3161 timestamp server, e.g.
  `http://timestamp.digicert.com`.

### 4. Linux

The default `bundle.targets: "all"` produces both `.deb` and `.AppImage`
on Linux. The `bundle.linux.deb.depends` list is currently empty;
add system packages here if Tauri's dependency detection misses any.

### 5. Auto-update

Not configured. The design says auto-update should be opt-in. When you
add it, do not enable check-on-launch by default — keep the
no-telemetry-out-of-the-box invariant.

## Verifying a build is clean

After `cargo tauri build`, the bundles land in
`target/release/bundle/<format>/`. Smoke checks before shipping:

1. Install the bundle on a clean VM.
2. Launch with the device unplugged. Welcome screen renders, status
   pill shows "No tablet". App is fully usable for browsing an existing
   library in read-only mode.
3. Plug the device in. Pill turns green. Connect → password dialog →
   sync. Library list populates.
4. Disconnect cable mid-sync. App reports the partial sync, library
   stays internally consistent (`Verify` reports zero issues).
5. Manually invoke the privacy invariant test (`cargo test --test
   no_egress` once it's added) to confirm the bundle attempts no
   external network destinations besides `10.11.99.1`.
