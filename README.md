# botc-mcp

Rust **MCP server** for models to play **Blood on the Clocktower** (Trouble Brewing
character set only): one fixed gameplay loop under a Storyteller moderator.

## Status

**Scaffold + rules documentation.** Engine and MCP tools not implemented yet.

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
