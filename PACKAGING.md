# Packaging Marginalia for distribution

Phase 1 ships unbundled — `cargo run -p rmsync-app --release` is the
supported way to launch the app. This document describes what's left
before producing signed installers for end users.

## Prerequisites

```sh
cargo install tauri-cli --version "^2.0.0"
```

Then `cargo tauri dev` runs the dev workflow with hot reload, and
`cargo tauri build` produces platform installers (once the items below
are addressed).

## What's left

### 1. Icons

The repository ships a placeholder 32×32 transparent PNG at
`crates/rmsync-app/icons/icon.png`. Real bundle builds need:

- `icons/32x32.png` (Linux deb / generic)
- `icons/128x128.png`
- `icons/128x128@2x.png` (256×256)
- `icons/icon.icns` (macOS — produced by `iconutil -c icns icon.iconset`)
- `icons/icon.ico` (Windows — multi-resolution ICO)

`tauri icon path/to/source-1024.png` will generate the full set if you
have a 1024×1024 master.

Once the icons are in place, set `"bundle.active": true` in
`crates/rmsync-app/tauri.conf.json`.

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
