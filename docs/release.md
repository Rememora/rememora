# Release process

Rememora uses **one project-wide SemVer version** for every shipped surface:

- Rust CLI crate (`Cargo.toml` and `Cargo.lock`)
- Claude Code plugin manifest (`plugin/.claude-plugin/plugin.json`)
- Claude Code plugin marketplace metadata (`.claude-plugin/marketplace.json`)
- GitHub release tag (`vX.Y.Z`)
- Homebrew formula version (updated by the release workflow)

`VERSION` at the repository root is the source of truth. The helper below updates every checked-in version surface together:

```bash
scripts/version.py set 1.2.3
```

CI verifies consistency with:

```bash
scripts/version.py --check
```

## Cutting a release

1. Create a release branch; never commit directly to `main`.
2. Run `scripts/version.py set X.Y.Z`.
3. Open and merge a PR with the version bump and any release notes/docs updates.
4. Tag the merge commit as `vX.Y.Z` and push the tag.
5. The `Release` workflow builds the CLI artifacts, creates the GitHub release, and dispatches the Homebrew tap update.

Do **not** create separate `plugin-v*` tags for new releases. The Claude Code plugin ships from the same `vX.Y.Z` project tag and carries the same manifest/marketplace version as the CLI.
