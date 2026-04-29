# Language Server (LSP) Setup

`boruna-lsp` implements the Language Server Protocol for `.ax` files.

## Build

```bash
cargo build --bin boruna-lsp
```

## VS Code

Install the [official LSP client extension](https://marketplace.visualstudio.com/items?itemName=boruna.boruna-lsp) (coming soon), or configure manually in `.vscode/settings.json`:

```json
{
  "boruna.lsp.serverPath": "/path/to/boruna-lsp"
}
```

Or use the generic `vscode-languageclient` setup with any LSP-capable extension.

## Neovim (nvim-lspconfig)

```lua
require('lspconfig').boruna_lsp.setup {
  cmd = { '/path/to/boruna-lsp' },
  filetypes = { 'ax' },
  root_dir = require('lspconfig.util').root_pattern('Cargo.toml', '.git'),
}
```

## Features

| Feature | Status |
|---------|--------|
| Real-time diagnostics (errors, warnings) | ✓ |
| Keyword and built-in completion | ✓ |
| Document formatting (`boruna fmt`) | ✓ |
| Hover (function signatures) | ✓ |
| Go-to-definition | planned |
| Rename symbol | planned |
