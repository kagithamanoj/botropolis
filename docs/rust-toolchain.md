# Rust toolchain setup

The Rust port of Botropolis builds with the stable Rust toolchain,
installed with [rustup](https://rustup.rs). No toolchain was present
on this machine before the port started; this is how it was set up and
how to repeat it.

## Install

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
```

That puts the toolchain manager in `~/.rustup/` and the binaries
(`cargo`, `rustc`, `rustup`, `cargo-clippy`, `cargo-fmt`) in
`~/.cargo/bin/`. Add that directory to `PATH` for the shell:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

The home directory persists across VM replacements, so the toolchain
survives a rebuild. `/tmp` does not; do not install it there.

Installed here: rustup 1.29.3, cargo 1.100.0, rustc 1.100.0
(stable-x86_64-unknown-linux-gnu).

## Update

```sh
rustup update stable
```

## Build and test

From the repo root:

```sh
cd rust
cargo test
cargo run -- roster
```

The workspace is `rust/Cargo.toml`; the crate is
`rust/botropolis/`. The binary is `botropolis`.

## Layout

- `rust/botropolis/src/` holds the Rust port: `schemas` (shared
  types), `models` (OpenAI/Anthropic/Google/Ollama/stub clients),
  `registry` (agent specs from `botropolis/agents/`), `agent`,
  `tools`, and later the loop, ledger, run state, CLI, and server.
- The Rust code loads the same agent YAML specs and serves the same
  web UI as the Python side. It does not duplicate them.
- The Python framework is untouched and its test suite stays green.
