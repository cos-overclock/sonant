# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Sonant is a CLAP audio plugin that generates MIDI patterns from natural language prompts and reference MIDI input. It uses LLM APIs (Anthropic Claude, OpenAI-compatible) to generate MIDI data displayed in a piano roll UI built with GPUI.

## Build and Development Commands

```bash
cargo build                                    # Debug build
cargo build --release                          # Release build
cargo test                                     # Run all tests
cargo test <test_name>                         # Run single test
cargo fmt                                      # Format code
cargo clippy --all-targets --all-features     # Lint
cargo run -- --gpui-helper                     # Standalone GPUI helper window
./scripts/build_clap_bundle.sh [debug|release] # Package macOS CLAP bundle to dist/Sonant.clap
```

## Architecture

### Binary Outputs

- **libsonant.dylib** (cdylib): The CLAP plugin loaded by DAWs
- **sonant** (rlib/bin): GPUI helper binary spawned by the plugin for GUI rendering

### Layer Structure

| Layer | Location | Responsibility |
|-------|----------|----------------|
| Plugin | `src/plugin/` | CLAP lifecycle, MIDI I/O, GUI bridge via `clap_adapter.rs` |
| Application | `src/app/` | Use case orchestration, generation jobs, MIDI routing |
| Domain | `src/domain/` | Generation modes, request validation, music theory types |
| Infrastructure | `src/infra/` | LLM clients (Anthropic/OpenAI), MIDI parsing, prompt building |
| UI | `src/ui/` | GPUI window, state management, piano roll rendering |

### Key Module Responsibilities

- **`plugin/clap_adapter.rs`**: CLAP entry point, extension registration (GUI, audio-ports, note-ports, state)
- **`domain/generation_contract.rs`**: `GenerationMode` enum (7 modes), `GenerationRequest` with mode-specific reference validation
- **`infra/llm/prompt_builder.rs`**: Mode-specific prompt templates, reference MIDI embedding for LLM input
- **`infra/llm/anthropic.rs` / `openai_compatible.rs`**: LLM API clients
- **`app/generation_service.rs`**: Orchestrates prompt building, API calls, response validation
- **`ui/window.rs`**: Main GPUI window with piano roll, mode selector, parameter controls
- **`ui/state.rs`**: UI state management, mode reference requirement checking

### Generation Modes (FR-05)

| Mode | Required Reference |
|------|--------------------|
| Melody, ChordProgression, DrumPattern, Bassline | None (optional references) |
| CounterMelody, Harmony | At least one Melody reference |
| Continuation | At least one reference of any type |

### Thread Model

- **Audio Thread**: DAW `process()` calls - must be non-blocking, no I/O
- **UI Thread**: GPUI rendering
- **Worker**: LLM API calls, MIDI parsing (async)

Communication uses lock-free queues between audio thread and app layer.

## Coding Conventions

- Rust 2024 edition with `rustfmt` defaults
- Keep audio-thread paths allocation-free and non-blocking
- LLM responses are JSON-only; schema validation in `infra/llm/schema_validator.rs`
- UI implementation follows reference images in `docs/image/`

## Testing

Integration tests in `tests/` cover FR (functional requirement) scenarios:
- `fr03a_reference_midi_flow.rs`: File-based MIDI reference
- `fr03b_live_input_reference_flow.rs`: Real-time MIDI input
- `fr04_generation_engine.rs`: LLM generation flow
- `fr05_mode_generation_flow.rs`: Mode-specific validation

## Documentation

- `docs/product.md`: Product requirements (FRs, NFRs, use patterns)
- `docs/software-architecture.md`: System architecture, layer definitions, FR mapping
- `docs/software-detailed-design.md`: Module interfaces, data structures, test design
