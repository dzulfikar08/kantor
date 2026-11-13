# Bundled spelling dictionaries

Kantor compiles these Hunspell dictionaries into its binary (`include_str!`
in `src/lib.rs`). Together they add about 1.0 MB to the executable (about
0.36 MB once compressed in the downloads); the checker code adds roughly
0.3 MB more. Loaded, each takes about 3 MB of memory.

Both were taken from the LibreOffice dictionaries repository
(https://github.com/LibreOffice/dictionaries, branch `master`) on 2026-11-13.

| Files | Language | Upstream | Licence |
|---|---|---|---|
| `en_US.aff`, `en_US.dic` | English (United States) | SCOWL / wordlist.sourceforge.net, version 2020.12.07; `.aff` as updated by Marco A.G. Pinto (2024-01-29) | SCOWL licence (permissive, MIT-like), with the BSD-style Ispell and WordNet 1.6 notices: see `README_en_US.txt` |
| `id_ID.aff`, `id_ID.dic` | Indonesian | hunspell-id (https://github.com/shuLhan/hunspell-id), based on Benitius Brevoort's Kamus Indonesia | LGPL-3.0-only: see `LICENSE-id_ID.txt` and `README_id_ID.adoc` |

Licence compatibility with Kantor (AGPL-3.0-or-later):

- The SCOWL, Ispell and WordNet terms only ask that their copyright and
  permission notices be kept, which `README_en_US.txt` and the top-level
  `NOTICE` do.
- LGPL-3.0 is GPL-3.0 with extra permissions, and GPL-3.0 and AGPL-3.0 may be
  combined (section 13 of each). The Indonesian files stay under LGPL-3.0;
  their source is these files, and anyone can rebuild Kantor with a changed
  dictionary since all of Kantor's source is available.

Kantor's only change: `id_ID.aff` says `SET UTF-8` instead of
`SET ISO8859-1`. Both files are pure ASCII apart from one typographic
apostrophe in `id_ID.dic` that was already UTF-8, so the declared encoding
now matches the bytes.
