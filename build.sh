#!/usr/bin/env bash
# Build the reHydrate desktop app.
#
# Usage:
#   ./build.sh          # release build, then launch the app
#   ./build.sh --debug  # faster compile, slower runtime; launches the app
#   ./build.sh --no-run # build only, do not launch
#   ./build.sh --skip-ui  # skip the UI bundle step (use cached ui/dist)

set -euo pipefail

cd "$(dirname "$0")"

PROFILE="release"
RUN=1
BUILD_UI=1

for arg in "$@"; do
  case "$arg" in
    --debug)    PROFILE="debug" ;;
    --no-run)   RUN=0 ;;
    --skip-ui)  BUILD_UI=0 ;;
    -h|--help)
      sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown flag: $arg" >&2
      exit 1
      ;;
  esac
done

echo "==> profile: $PROFILE"

if [[ $BUILD_UI -eq 1 ]]; then
  echo "==> building UI bundle (ui/dist)"
  pushd ui >/dev/null
  if [[ ! -d node_modules ]]; then
    # `npm ci` enforces lockfile parity; if the lock has drifted from
    # `package.json` we want a hard failure here rather than a silent
    # regenerate.
    npm ci
  fi
  npm run build
  popd >/dev/null
else
  echo "==> skipping UI build (--skip-ui)"
fi

CARGO_FLAGS=(-p rehydrate-app)
if [[ "$PROFILE" == "release" ]]; then
  CARGO_FLAGS+=(--release)
fi

if [[ $RUN -eq 1 ]]; then
  echo "==> cargo run ${CARGO_FLAGS[*]}"
  cargo run "${CARGO_FLAGS[@]}"
else
  echo "==> cargo build ${CARGO_FLAGS[*]}"
  cargo build "${CARGO_FLAGS[@]}"
  if [[ "$PROFILE" == "release" ]]; then
    BIN="target/release/rehydrate-app"
  else
    BIN="target/debug/rehydrate-app"
  fi
  echo "==> built: $BIN"
fi
