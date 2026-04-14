# Qualitool — top-level command runner (ADR-0002)

# Build all Rust crates
build:
    cargo build --workspace

# Run all tests (Rust + TypeScript)
test:
    cargo test --workspace
    pnpm -r run --if-present test

# Format Rust code
fmt:
    cargo fmt --all

# Lint all Rust crates
lint:
    cargo clippy --workspace -- -D warnings

# Regenerate TypeScript types from qualitool-protocol
codegen:
    @echo "codegen: not yet wired (see QUA-16)"

# Run the qualitool CLI (pass args after --)
run *ARGS:
    cargo run --bin qualitool -- {{ARGS}}

# Start the qualitool HTTP/WebSocket server
serve *ARGS:
    cargo run --bin qualitool -- serve {{ARGS}}

# Remove build artifacts
clean:
    cargo clean
    rm -rf packages/*/dist packages/*/node_modules
