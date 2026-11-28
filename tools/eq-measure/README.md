# Equation engine cost (ADR 0008)

Three throwaway binaries measured on a GitHub runner with Kantor's release
profile (fat LTO, one codegen unit, stripped): `base` (ttf-parser only),
`mitexm` (MiTeX's LaTeX → Typst converter) and `typstm` (Typst 0.15
compiling two display equations). Each is its own workspace:

```sh
cd tools/eq-measure/typstm && cargo build --release && ls -l target/release/typstm
```

Results (2026-11-28, ubuntu-24.04): base 0.34 MB, MiTeX 0.62 MB, Typst
31.3 MB (279 s build). See docs/decisions/0008-equations.md.
