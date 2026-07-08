# botc-mcp

Rust **eval MCP server** for [Blood on the Clocktower](https://bloodontheclocktower.com/)
gameplay scenarios. Agents can retrieve structured rules and (later) run
scripted evaluations against a rules engine.

## Status

**Scaffold + rules documentation.** Game engine and MCP tools are not implemented yet.

## Documentation

Authoritative-for-this-repo rules live under [`docs/`](docs/README.md), split for
retrieval:

- Gameplay loop, setup, win conditions, voting
- States (drunk/poisoned), ability meta-rules, night order
- Full **Trouble Brewing** role reference

Start at [`docs/README.md`](docs/README.md).

## Development

```bash
cargo build
cargo test
cargo run
```

Requires a recent stable Rust toolchain (edition 2024).

## License

Code: TBD.  
Rules text: paraphrased community/official reference; game © The Pandemonium Institute.
