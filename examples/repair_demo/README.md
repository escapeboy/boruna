# Repair Demo

Demonstrates the structured diagnostics and repair workflow.

`broken.ax` contains three intentional errors:

1. **E005** Non-exhaustive match: `update` only handles `Increment`, missing `Decrement` and `Reset`
2. **E006** Wrong field name: `countt` instead of `count` in `init`
3. **E007** Capability violation: `update` declares `!{fs.read}` but must be pure

## Usage

```
# Check for errors (human-readable)
boruna lang check examples/repair_demo/broken.ax

# Check for errors (JSON)
boruna lang check examples/repair_demo/broken.ax --json

# Auto-repair with best suggestions
boruna lang repair examples/repair_demo/broken.ax --apply best
```
