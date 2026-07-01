# Contributing to vpsguard

Thanks for helping. This guide keeps changes consistent and safe.

## Getting started

```sh
git clone https://github.com/KodjoTouglo/hardn.git
cd hardn
cargo build --workspace
cargo test --workspace
```

Rust 1.85 or newer.

## Workflow

- Branch from `develop`, one feature per branch, prefixed `feature/<slug>`.
- Open the pull request against `develop`.
- Keep pull requests focused; smaller is easier to review.

## Before you push

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

All three must be clean. CI runs the same checks.

## Code conventions

- Comments are terse: short, no filler, no em-dash. One line per struct field.
- Every module implements the `Module` trait
  (`check`/`plan`/`apply`/`rollback`/`uninstall`).
- Actions are idempotent, support dry-run, and are reversible.
- Do all system access through `Context` (its command runner and filesystem),
  never `std::fs` or `std::process` directly, so modules stay testable and work
  remotely.
- Unit-test the pure logic; integration-test apply/rollback against a tempdir
  with a mock runner. No test may touch the real host.

## Adding a module

1. Add its config to `vpsguard-core` (`config.rs`) and export it.
2. Implement the module in `vpsguard-modules`, register it in the catalog.
3. Add it to the example config and cover it with tests.
4. Make it cross-distro via `Platform` (service names, package managers).

## Reporting bugs and ideas

Use the issue templates. For security issues, follow
[SECURITY.md](SECURITY.md), do not open a public issue.

By contributing you agree your work is licensed under Apache-2.0.
