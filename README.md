# pycoati

A new tool.

## Install

```bash
# From source (Rust)
cargo install --path .

# Or as a Python package (via maturin)
maturin develop
```

## Usage

```bash
pycoati --help
```

## Development

```bash
# Pre-commit checks
cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features

# Full review (fmt, clippy, tests, audit, deny)
make review
```

## License

MIT
