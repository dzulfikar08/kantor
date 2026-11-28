# Decision records

One file per decision: `NNNN-short-title.md` with **Context**, **Options**, **Decision**, **Consequences**.

- [0001 — Text engine for Docs](0001-text-engine.md): parley for layout, GPUI for painting.
- [0002 — Cloud storage](0002-cloud-storage.md): plain WebDAV client (`kantor_cloud`); Nextcloud as the default server, OpenCloud supported.
- [0003 — PDF renderer](0003-pdf-renderer.md): hayro (pure Rust) draws pages, extracts text and reads outlines and links for the PDF viewer.
- [0004 — e-meterai and certified signatures](0004-esign-providers.md): VIDA and Mekari Sign (e-meterai) and Xignature (hash signing) from their public API docs, others as stubs (`kantor_esign`).
- [0005 — Notes editor](0005-notes-editor.md): a drawn editor over the Markdown text (pulldown-cmark line model, GPUI-shaped lines, hidden markup except on the caret's lines), byte-exact saves.
- [0008 — Equations](0008-equations.md): an own OpenType MATH layout engine (`kantor_math`, STIX Two Math bundled) and LaTeX parser, OMML kept byte for byte; Typst measured at +31 MB and set aside.
