# Repository Guidelines

## Project Structure & Module Organization
- `src/lib.rs` exposes the plugin library crate; plugin integration lives in `src/plugin/`.
- `src/plugin/clap_adapter.rs` contains the CLAP entry point, extension registration, GUI bridge, and audio/state hooks.
- `src/main.rs` is the GPUI helper binary entry (`--gpui-helper`) used by the plugin GUI flow.
- `scripts/build_clap_bundle.sh` builds and assembles the macOS CLAP bundle into `dist/Sonant.clap/Contents/`.
- `docs/` contains product and architecture references (`product.md`, `software-architecture.md`, `software-detailed-design.md`).
- `target/` is build output; do not commit artifacts from it.

## Build, Test, and Development Commands
- `cargo build` builds debug artifacts for library + helper binary.
- `cargo build --release` builds optimized artifacts.
- `cargo test` runs unit/integration tests.
- `cargo fmt` formats the codebase with Rust defaults.
- `cargo clippy --all-targets --all-features` runs lint checks across targets.
- `cargo run -- --gpui-helper` starts the standalone GPUI helper window for local GUI checks.
- `./scripts/build_clap_bundle.sh [debug|release]` packages `libsonant.dylib` and `SonantGUIHelper` into `dist/Sonant.clap`.

## Coding Style & Naming Conventions
- Use Rust 2024 idioms and `rustfmt` defaults (4-space indentation, trailing commas where appropriate).
- Naming: `snake_case` for modules/functions, `PascalCase` for types/traits, `UPPER_SNAKE_CASE` for constants.
- Keep audio-thread paths non-blocking; avoid network/file I/O in real-time processing code.
- Prefer small, focused modules under `src/plugin/` when splitting plugin responsibilities.

## Testing Guidelines
- Place fast unit tests close to implementation (`#[cfg(test)]`), and integration tests under `tests/` as coverage grows.
- Name tests by behavior, e.g. `load_accepts_empty_state_payload`.
- Validate before opening a PR: `cargo fmt`, `cargo clippy --all-targets --all-features`, `cargo test`.

## Commit & Pull Request Guidelines
- Follow the current history style: short, imperative commit subjects (e.g., `Add ...`, `Fix ...`), ideally under 72 characters.
- Keep commits logically scoped (feature, refactor, and formatting changes separate).
- PRs should include: purpose, key changes, verification steps/command output, and linked issue(s).
- For GUI or bundle changes, add host/OS details and a screenshot or log snippet from local validation.
