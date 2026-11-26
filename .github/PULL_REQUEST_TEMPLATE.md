## What this changes

<!-- One problem per pull request. Describe the problem and the fix. -->

## Tested on

| | |
| --- | --- |
| GPU and VRAM | |
| GPU driver | |
| CPU | |
| RAM | |
| Windows build | |
| Console mode and launcher flags | |

## Before and after

<!-- Same scene, same settings, same driver. Frame times (median and 95th percentile), committed
memory, dedicated GPU memory, and anything else the change targets. -->

| Measurement | Before | After |
| --- | --- | --- |
| | | |

## Checklist

- [ ] `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass.
- [ ] AMD paths still work (or the change does not touch them).
- [ ] If this ports code from another fork or project, the source is credited here and in the commit message, and its license is compatible with GPL-3.0-or-later.
- [ ] No game names, title files or anything derived from them (see docs/asset-policy.md).
