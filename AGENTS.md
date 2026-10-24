# AGENTS.md

## Project mantra

screencapture is lightweight, std-only Rust. Keep it that way.

- Use `std` and hand-written platform FFI. Do not add crate dependencies;
  `Cargo.toml` has no `[dependencies]` table and should stay that way.
- Keep `cargo fmt --all --check` clean.
- Keep `cargo clippy --all-targets --locked -- -D warnings` clean. The manifest
  denies `clippy::all`, `clippy::pedantic`, and `unsafe_op_in_unsafe_fn`.
- Do not add `#[allow(...)]` blocks or other `allow` lint attributes. Fix the
  warning, or use a narrowly justified `#[expect(...)]` only when the lint is
  genuinely unavoidable.
- Run `cargo test --all-targets --locked`. Keep the supported cross-target
  clippy matrix warning-free too.
- Prefer small modules, explicit types, and native platform code over new
  machinery.

## Commits

- Keep one logical change per commit. Stage the files that belong to it by
  path (`git add <paths>`), and leave unrelated or other agents' edits for
  their own commit.
- Never bulk-stage or bulk-commit the tree (`git add -A`, `git add .`,
  `git commit -a`). Split the work into focused commits instead.

## Attribution

The maintainer uses an AI coding harness. It is a tool, not a contributor.

- Never add co-author trailers for tools, generated-with footers, robot
  signatures, or any other harness attribution to commits, tags, pull requests,
  release notes, comments, or documentation.
- Do not add any tool or harness to contributor lists, `.mailmap`, authorship
  metadata, or release credits.
- Commit and tag messages should describe the change only, with no tool
  attribution.
- If a tool or template adds attribution automatically, remove it before
  committing.
