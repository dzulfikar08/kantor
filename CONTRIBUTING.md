# Contributing

Thanks for helping! Short version:

1. Open an issue first for anything bigger than a bug fix.
2. Keep the architecture rules from [plan.md §4.3](plan.md): model / layout / view are separate,
   and crates other than `*_ui` and `kantor` must not depend on GPUI.
3. **Never delete what we don't understand** in a file (plan.md §5). Round-trip tests are required
   for format code.
4. CI must be green: `cargo fmt`, `cargo clippy -D warnings`, `cargo test` on x86_64 and aarch64.
5. Test documents go in `tests/corpus/` only with a known licence, recorded in its README.
   Never commit private documents.

You don't need to build locally: push a branch and CI builds and tests it on both architectures.
Contributions are licensed under AGPL-3.0-or-later, as the project is.
