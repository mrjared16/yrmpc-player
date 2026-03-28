# CLAUDE.md

## Local `rmpc/` rules

- This directory is the Cargo root for the app.
- Prefer `cargo nextest run` over `cargo test` for normal verification.
- Never run `cargo fmt` unless the user explicitly asks for it.
- Avoid broad formatting-only churn while implementing behavior changes.

## Typical verification

- `cargo nextest run <targeted tests>` while iterating
- `cargo clippy` for broader validation when needed
