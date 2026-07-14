## Summary

<!-- What does this change do and why? -->

## Scope

- [ ] Behaviour or API change
- [ ] Documentation only
- [ ] Tests or tooling
- [ ] Security-sensitive change

## Verification

<!-- List the commands you ran and their results. -->

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
```

## Compatibility and risk

<!-- Mention wire/storage/API compatibility, migrations, security impact, or known limitations. -->
