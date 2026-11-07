# Kantor patches to gpui-pre-wgpu 0.3.11 (Apache-2.0)

Vendored from crates.io `gpui-pre-wgpu` 0.3.11 and wired in with `[patch.crates-io]`
in the workspace `Cargo.toml`. Changes (search for `KANTOR PATCH`):

1. Path MSAA is opt-in (`KANTOR_PATH_MSAA=4`); default sample count 1.
2. The window-sized path intermediate/MSAA textures are allocated only on frames that
   draw paths and destroyed otherwise.

Why: on an 8 GB M1 under Asahi, wgpu hit `Out of Memory` / device lost under memory
pressure. Same idea as VoltrusID/gpui commits 64c8ce4 and bd450ea (Metal).
Drop this vendor copy once upstream frees these textures itself.
