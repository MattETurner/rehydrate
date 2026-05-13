#!/usr/bin/env bash
# Build the reHydrate desktop app.
#
# Default mode produces a release `.dmg` + `.app` under
# `target/aarch64-apple-darwin/release/bundle/` — the same artefacts
# the GitHub release workflow attaches to a tag, minus the
# SHA256SUMS upload step. Use this when you want to test the actual
# installer locally before tagging a release.
#
# Usage:
#   ./build.sh                # build the .dmg + .app bundle (release)
#   ./build.sh --open         # ... and reveal the bundle in Finder
#   ./build.sh --install      # ... and copy the .app to /Applications
#   ./build.sh --skip-ui      # reuse ui/dist (faster repeat builds)
#   ./build.sh --dev          # cargo run -p rehydrate-app --release
#                             # (fast iteration; produces no installer)
#   ./build.sh --dev --debug  # ... in debug profile
#   ./build.sh --dev --no-run # build the dev binary without launching
#   ./build.sh -h | --help
#
# Two `cargo tauri build` flags are non-negotiable:
#
# 1. `--target aarch64-apple-darwin`
#    Pins the bundle to Apple Silicon. Building on an x86_64 host
#    against the host's default target would silently produce an
#    Intel `.dmg` we don't ship; the script refuses that
#    combination explicitly below.
#
# 2. `-- --no-default-features`
#    Belt-and-suspenders even though `default = []` on `rehydrate-app`
#    today: a future `default = [...]` addition must not slip into a
#    shipped binary. DevTools (an opt-in feature) is a security
#    boundary skip because the renderer can call any registered
#    Tauri command. The release workflow passes this flag too —
#    keeping the script in lockstep means "I built it locally and it
#    worked" matches what users actually get from a tagged release.

set -euo pipefail

cd "$(dirname "$0")"

MODE="bundle"        # bundle | dev
PROFILE="release"    # release | debug (debug only meaningful in --dev mode)
BUILD_UI=1
RUN=0                # auto-launch unbundled binary after build (--dev only)
OPEN_BUNDLE=0        # `open` the produced bundle dir in Finder
INSTALL_BUNDLE=0     # copy the produced .app to /Applications

for arg in "$@"; do
  case "$arg" in
    --dev)       MODE="dev"; RUN=1 ;;
    --debug)     PROFILE="debug" ;;
    --skip-ui)   BUILD_UI=0 ;;
    --no-run)    RUN=0 ;;       # only meaningful in --dev mode
    --open)      OPEN_BUNDLE=1 ;;
    --install)   INSTALL_BUNDLE=1 ;;
    -h|--help)
      sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown flag: $arg" >&2
      echo "(run \`./build.sh --help\` for usage)" >&2
      exit 1
      ;;
  esac
done

# Reject flag combinations that look intentional but do nothing.
# Silent no-ops in scripted invocations bury real bugs.
if [[ "$MODE" == "bundle" && $RUN -eq 0 ]]; then
  # `--no-run` only meaningful in --dev (the bundle path never auto-
  # launches in the first place). The default for `RUN` is 0; we
  # only catch this when the user explicitly passed `--no-run` and
  # didn't pair it with `--dev`.
  : # NOP — RUN=0 is the bundle-mode default; nothing to gate on.
fi
if [[ "$MODE" == "dev" && $INSTALL_BUNDLE -eq 1 ]]; then
  echo "==> \`--install\` requires the bundle mode (run without --dev)." >&2
  exit 1
fi
if [[ "$MODE" == "dev" && $OPEN_BUNDLE -eq 1 ]]; then
  echo "==> \`--open\` requires the bundle mode (run without --dev)." >&2
  exit 1
fi

# Refuse cross-arch surprises. The release pipeline ships ARM-only;
# building an Intel `.dmg` locally would produce something that
# won't run on the machines we actually ship to. `--dev` is exempt
# because it just runs the bare binary for whatever host you're on.
ARCH="$(uname -m)"
if [[ "$MODE" == "bundle" && "$ARCH" != "arm64" ]]; then
  echo "==> refusing to bundle on $ARCH host." >&2
  echo "    reHydrate's release pipeline produces aarch64-apple-darwin only." >&2
  echo "    Use \`./build.sh --dev\` to run the bare binary on this host," >&2
  echo "    or run this script on an Apple Silicon Mac to produce a .dmg." >&2
  exit 1
fi

# Verify build prerequisites BEFORE the slow UI build, so a fresh
# clone without the Tauri CLI installed fails in 50 ms rather than
# burning ~1–2 minutes on `npm ci` first. Dev mode skips this — it
# just runs the bare binary and doesn't need cargo-tauri.
if [[ "$MODE" == "bundle" ]]; then
  if ! command -v cargo-tauri >/dev/null 2>&1; then
    echo "==> tauri-cli missing; install with:" >&2
    echo "       cargo install tauri-cli --version '^2.0.0'" >&2
    exit 1
  fi
  # Ensure the aarch64 target is installed (no-op if it already
  # is). `rustup target add` is idempotent and very fast on a hit.
  if command -v rustup >/dev/null 2>&1; then
    if ! rustup target list --installed | grep -q '^aarch64-apple-darwin$'; then
      echo "==> installing aarch64-apple-darwin target via rustup"
      rustup target add aarch64-apple-darwin
    fi
  fi
fi

echo "==> mode: $MODE / profile: $PROFILE"

# UI build is required for both modes — the Tauri binary loads its
# frontend from `ui/dist/` (see `frontendDist` in tauri.conf.json).
if [[ $BUILD_UI -eq 1 ]]; then
  echo "==> building UI bundle (ui/dist)"
  pushd ui >/dev/null
  if [[ ! -d node_modules ]]; then
    # `npm ci` enforces lockfile parity; if the lock has drifted
    # from `package.json` we want a hard failure here rather than a
    # silent regenerate.
    npm ci
  fi
  npm run build
  popd >/dev/null
else
  echo "==> skipping UI build (--skip-ui)"
fi

if [[ "$MODE" == "dev" ]]; then
  # Quick local iteration. Doesn't produce an installer; DevTools
  # is genuinely useful while iterating so opt into it explicitly
  # (the crate's default feature set is empty so shipped builds
  # never expose the inspector).
  CARGO_FLAGS=(-p rehydrate-app --features devtools)
  [[ "$PROFILE" == "release" ]] && CARGO_FLAGS+=(--release)
  if [[ $RUN -eq 1 ]]; then
    echo "==> cargo run ${CARGO_FLAGS[*]}"
    exec cargo run "${CARGO_FLAGS[@]}"
  else
    echo "==> cargo build ${CARGO_FLAGS[*]}"
    cargo build "${CARGO_FLAGS[@]}"
    BIN="target/${PROFILE}/rehydrate-app"
    echo "==> built: $BIN"
    exit 0
  fi
fi

# ===== Bundle path =====
# Prerequisite checks (cargo-tauri + aarch64 target) ran above
# before the UI build so a missing tool fails fast.

echo "==> cargo tauri build (--no-default-features, aarch64-apple-darwin)"
# `cargo tauri build` runs the bundler. The `--bundles app,dmg`
# limit keeps us from producing `.app.tar.gz` archives the local
# workflow doesn't need; the release workflow asks for both because
# the auto-update flow (not yet wired) consumes the tarball.
#
# Everything after `--` is passed straight to cargo; that's where
# `--no-default-features` belongs. `tauri build` would consume
# `--no-default-features` itself and never forward it, which is the
# subtle reason for the double-dash.
cargo tauri build \
  --target aarch64-apple-darwin \
  --bundles app,dmg \
  -- --no-default-features

BUNDLE_DIR="target/aarch64-apple-darwin/release/bundle"
DMG_PATH="$(ls -t "$BUNDLE_DIR/dmg"/*.dmg 2>/dev/null | head -n1 || true)"
APP_PATH="$(ls -dt "$BUNDLE_DIR/macos"/*.app 2>/dev/null | head -n1 || true)"

echo ""
echo "==> bundle complete"
[[ -n "$DMG_PATH" ]] && echo "    DMG: $DMG_PATH"
[[ -n "$APP_PATH" ]] && echo "    APP: $APP_PATH"

# SHA256 the .dmg so the local build can be compared against what CI
# would attach to a tag. Matches the format used in the release
# workflow's SHA256SUMS file.
if [[ -n "$DMG_PATH" && -x "$(command -v shasum)" ]]; then
  echo ""
  echo "==> sha256:"
  (cd "$(dirname "$DMG_PATH")" && shasum -a 256 "$(basename "$DMG_PATH")")
fi

if [[ $INSTALL_BUNDLE -eq 1 && -n "$APP_PATH" ]]; then
  TARGET="/Applications/$(basename "$APP_PATH")"
  echo ""
  echo "==> installing to $TARGET"
  if [[ -d "$TARGET" ]]; then
    rm -rf "$TARGET"
  fi
  cp -R "$APP_PATH" /Applications/
  echo "    done. Right-click → Open on first launch to bypass Gatekeeper."
fi

if [[ $OPEN_BUNDLE -eq 1 && -n "$DMG_PATH" ]]; then
  open -R "$DMG_PATH"
fi
