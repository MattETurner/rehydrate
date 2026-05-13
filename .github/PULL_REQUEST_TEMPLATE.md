<!--
Thanks for the patch! A short PR description goes a long way.
The checklist below mirrors what CI enforces — please tick what applies.
-->

## Summary

<!-- One or two sentences: what changes and why. -->

## Related issue

<!-- Closes #123 / Refs #123 / N/A -->

## How to test

<!-- Steps a reviewer can run locally. If the change is UI-only, a
short screen recording or screenshot is great. -->

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace --no-fail-fast` passes
- [ ] UI (if touched): `tsc --noEmit`, `eslint .`, and `npm run build` all clean
- [ ] `CHANGELOG.md` updated under `[Unreleased]` (or N/A for chore/CI-only PRs)
- [ ] No new TODO/FIXME comments left in the diff
- [ ] No secrets, tokens, or unredacted credentials in code, tests, or logs
