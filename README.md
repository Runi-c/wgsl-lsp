# WGSL-LSP

This project is a WIP WGSL language server that hooks directly into [naga](https://github.com/gfx-rs/wgpu/tree/trunk/naga) and supports [naga_oil](https://github.com/bevyengine/naga_oil) extensions such as [preprocessor directives](https://github.com/bevyengine/naga_oil/blob/33e57e488660aaeee81fa928454e51c215f9d0be/readme.md#preprocessing) and [imports](https://github.com/bevyengine/naga_oil/blob/33e57e488660aaeee81fa928454e51c215f9d0be/readme.md#imports).

### Architecture

`wgsl-lsp` is based on the [`async_lsp`](https://github.com/oxalica/async-lsp) framework. This was chosen instead of the more popular [`tower-lsp`](https://github.com/ebkalderon/tower-lsp) because I found that crate to be significantly unwieldy and it suffers from [handler execution order issues](https://github.com/ebkalderon/tower-lsp/issues/284) due to everything being forced `async fn`, which mandates async synchronization structures over all server state. Other projects chose [`rust-analyzer`](https://github.com/rust-lang/rust-analyzer/tree/f216be4a0746142c5f30835b254871256a7637b8/lib/lsp-server)'s [`lsp-server`](https://crates.io/crates/lsp-server) for [similar reasons](https://github.com/astral-sh/ruff/pull/10158), but I found that crate to be far too low-level compared with [`async_lsp`](https://github.com/oxalica/async-lsp) which solves the same issue.

### Naga/Naga-Oil Pain Points (as of April 2024)

- [[issue]](https://github.com/gfx-rs/wgpu/issues/5295) - Naga can only return a single validation error at a time, which severely limits the information we can present via diagnostics.
- [no issue yet] - Naga modules can only be built from already-valid source, meaning all other language features (go to definition, hover, function signature help, semantic highlighting, etc) cease functioning if there's a single error.
- [no issue yet] - Naga [`Module`](https://docs.rs/naga/0.19.2/naga/struct.Module.html)s contain lots of spans and semantic information, but it's incomplete in many ways for semantic highlighting. [`Function`](https://docs.rs/naga/0.19.2/naga/struct.Function.html)s have spans corresponding to their entire definition, but the spans for the name, arguments, argument types, and return type must all be parsed out manually. Similarly, [`Expression`](https://docs.rs/naga/0.19.2/naga/enum.Expression.html)s contain a lot of useful info, but they stop short of being very useful for semantic highlighting due to lack of sub-spans or any kind of AST. One example is when an [`Expression::Math`](https://docs.rs/naga/0.19.2/naga/enum.Expression.html#variant.Math) refers to a function parameter, that reference tells you nothing about where the function parameter appears in the expression source code.
- [[issue]](https://github.com/bevyengine/naga_oil/issues/76) - Naga-Oil doesn't make it easy to determine error source locations when constructing modules.
- [no issue yet] - Naga-Oil can only report a single error for an entire dependency tree. If there's an error in any ancestor, the current file you're looking at can't be validated.

### How to Run on VSCode

1. `cargo install --path ./` in this directory to install `wgsl-lsp` as a binary
2. Clone a language client extension that can point to `wgsl-lsp` - there's a minimal one here: https://github.com/Runi-c/wgsl-lsp-client
3. Open that folder in vscode and hit F5 to launch the extension in an extension host window
4. Open a workspace containing one or more WGSL files in the extension host window and the extension should activate automatically and start providing diagnostics
