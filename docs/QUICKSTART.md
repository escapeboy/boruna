# Boruna Quick Start

Boruna — Deterministic Agent-Native Execution Platform

## Build

```bash
cargo build --workspace
```

## Run a Program

```bash
cargo run --bin boruna -- run examples/hello.ax
```

## Create a Framework App

```bash
cargo run --bin boruna -- framework new my-app
cargo run --bin boruna -- run my-app/my-app.ax
```

## Run Tests

```bash
cargo test --workspace
```

## Key Commands

```bash
# Compile to bytecode
boruna compile app.ax

# Run with capability policy
boruna run app.ax --policy allow-all

# Framework validation
boruna framework validate app.ax

# Framework interactive test
boruna framework test app.ax -m "increment:1,reset:0"

# Run with real HTTP (requires http feature)
cargo run --features boruna-cli/http --bin boruna -- run app.ax --policy allow-all --live

# Run a workflow with real HTTP
cargo run --features boruna-cli/http --bin boruna -- workflow run my-workflow/ --policy allow-all --live

# Diagnostics
boruna lang check app.ax --json

# Auto-repair
boruna lang repair app.ax

# Templates
boruna template list
boruna template apply crud-admin --args "entity_name=products,fields=name|price"

# Package management
boruna-pkg init
boruna-pkg add std-ui 0.1.0
boruna-pkg install

# Orchestration
boruna-orch plan spec.json
boruna-orch status
```
