# Hello, Literate Boruna

This is a minimal demonstration of `boruna literate extract`. The single
markdown file below is BOTH a human-readable description AND the executable
spec. Running `boruna literate extract examples/literate/hello_literate.md`
produces a runnable `.ax` file under `gen/`.

The pattern matches Quint's Literate Specifications convention: code fences
labeled `<lang> <filename> +=` are appended to the named output file.

## The program

We want a tiny module that exposes a greeting constant and a `main` that
returns success. The greeting itself is a constant:

```ax hello.ax +=
fn greeting() -> String {
    "Hello, literate Boruna!"
}
```

`main` returns zero — Boruna treats this as exit-success:

```ax hello.ax +=

fn main() -> Int {
    0
}
```

## What this isn't

Non-Boruna fences in this document are ignored. For example, this Rust
snippet won't be extracted:

```rust
fn main() {
    println!("not extracted");
}
```

And a bare `ax` fence without `+=` is treated as a syntax-highlighted
prose example, not an extraction target:

```ax
let example_in_prose = 1
```

## Verifying

After extraction the produced file is a regular `.ax` source that compiles
and runs:

```sh
boruna literate extract examples/literate/hello_literate.md
boruna run gen/hello.ax
```
