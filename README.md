# botc-mcp

Rust **MCP server** for models to play **Blood on the Clocktower** (Trouble Brewing
character set only): one fixed gameplay loop under a Storyteller moderator.

## Status

**Rules docs + engine/MCP architecture sketch.** Core loop not fully implemented yet.

Design: [`docs/architecture.md`](docs/architecture.md). Rust modules under `src/` (`auth`, `comms`, `game`, `roles`, `tools`).

## Rules

Simulation rules live under [`docs/`](docs/README.md):

- Gameplay loop, setup, win conditions, voting
- States (drunk/poisoned), ability resolution, night order
- Full role reference for this character pool

## Development

```bash
cargo build
cargo test
cargo run
```

Requires a recent stable Rust toolchain.

## License

Code: TBD.  
Rules text: paraphrased reference; game © The Pandemonium Institute.
