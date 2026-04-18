# Contributing to Rememora

Thanks for your interest in contributing!

## Getting Started

```bash
git clone https://github.com/Rememora/rememora.git
cd rememora
cargo test
cargo build
```

## Development

- **Tests**: `cargo test` — all tests use in-memory SQLite, no setup needed
- **Lint**: `cargo clippy`
- **Format**: `cargo fmt`

## Pull Requests

1. Fork the repo and create a feature branch
2. Make your changes with tests
3. Run `scripts/version.py --check && cargo test && cargo clippy`
4. Open a PR against `main`

## Releases

Rememora uses one project-wide version for the CLI, Homebrew formula, and Claude Code plugin. See [`docs/release.md`](../docs/release.md) for the release checklist. Use `scripts/version.py set X.Y.Z` to bump all checked-in version surfaces together.

## Issues

Check the [project board](https://github.com/orgs/Rememora/projects/3) for current priorities. Issues labeled `Ready-For-Dev` are good to pick up.
