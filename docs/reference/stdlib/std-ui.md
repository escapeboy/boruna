# std-ui

> Declarative UI primitives for framework apps

**Package:** `std.ui`  **Version:** `0.1.0`  **Capabilities required:** none

## Overview

`std-ui` provides a small set of pure functions that return `UINode` values — the building blocks of a Boruna framework app's view layer. Every function is deterministic and side-effect-free. Wire these together in your `view(State) -> UINode` implementation to compose layouts, controls, and data-display elements.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.ui": "0.1.0"
```

## API Reference

### Types

#### `UINode`

```
type UINode { tag: String, text: String }
```

The universal tree node returned by every `std-ui` function. The `tag` identifies the element kind; `text` carries the primary display string. The framework runtime interprets these fields when rendering.

### Functions

#### Layout

##### `row(child1: UINode, child2: UINode) -> UINode`

Places two nodes side-by-side in a horizontal row.

##### `column(child1: UINode, child2: UINode) -> UINode`

Stacks two nodes vertically.

##### `stack(child1: UINode, child2: UINode) -> UINode`

Overlays two nodes in a Z-stack (front-to-back).

#### Containers

##### `container(content: UINode) -> UINode`

Wraps a node in a generic container — useful for spacing or styling boundaries.

##### `card(title: String, content: UINode) -> UINode`

Renders a titled card surface around `content`.

**Parameters**
- `title` — display title shown at the top of the card
- `content` — the inner `UINode` to render inside

##### `section(heading: String, content: UINode) -> UINode`

A named section with a visible heading above `content`.

#### Controls

##### `button(label: String, on_click: String) -> UINode`

An interactive button.

**Parameters**
- `label` — text displayed on the button
- `on_click` — message tag dispatched when clicked

**Example**
```
fn main() -> Int {
  let btn: UINode = button("Save", "save_clicked")
  0
}
```

##### `input(name: String, value: String, on_change: String) -> UINode`

A text input field.

**Parameters**
- `name` — field identifier
- `value` — current value to display
- `on_change` — message tag dispatched on every change

##### `select_field(name: String, selected: String, on_change: String) -> UINode`

A dropdown select field.

##### `checkbox(name: String, checked: Int, on_change: String) -> UINode`

A checkbox. `checked: 1` renders the box checked; `0` renders it unchecked.

#### Data display

##### `table_view(headers: String, row_count: Int) -> UINode`

A tabular view. `headers` is a comma-separated list of column names. `row_count` indicates how many rows the runtime should render.

##### `error_display(message: String) -> UINode`

Shows an error message in a styled error block.

##### `text(content: String) -> UINode`

Renders a plain text node.

##### `badge(label: String, variant: String) -> UINode`

A small inline badge. `variant` hints at the visual style (e.g. `"success"`, `"warning"`, `"error"`).

#### Helpers

##### `empty_node() -> UINode`

A no-op placeholder node that renders nothing. Use when a conditional branch needs to return a `UINode` without visible output.

##### `divider() -> UINode`

A horizontal rule / visual separator.

## Capabilities

None. All functions are pure and return data structures only — no I/O occurs during view construction.

## Notes / Limitations

- `UINode` currently carries only `tag` and `text`. Attributes such as CSS classes, event payloads, and children beyond the first two are not yet representable in the type. This is a v0.x constraint; the type will be expanded in a future release.
- `row`, `column`, and `stack` each accept exactly two children. Composing more requires nesting calls.
- `string_length` and other built-ins used internally are resolved by the Boruna runtime.
