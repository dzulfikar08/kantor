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

Kantor's changes (both files stay under LGPL-3.0):

- `id_ID.aff` says `SET UTF-8` instead of `SET ISO8859-1`. Both files are
  pure ASCII apart from one typographic apostrophe in `id_ID.dic` that was
  already UTF-8, so the declared encoding now matches the bytes.
- 904 entries were removed from `id_ID.dic` (2026-11-14); they are listed in
  `id_ID-removed.tsv` with the everyday word each one shadows.
  hunspell-id lists every headword of an old general dictionary, including
  archaic, regional and non-standard spellings that nobody writes today
  (`tingi`, `masarakat`, `semejak`, `jereka`, `katena`). Being entries, they
  were accepted, and many sit one keystroke away from very common words, so
  typos such as `tingi` for `tinggi` or `todak` for `tidak` passed. They
  were not produced by the affix rules: the `.aff` file is unchanged.
  `../tools/tighten_id.py` finds them: bare headwords (no affix flags)
  that are one typical typo (a dropped, doubled or undoubled letter, two
  letters swapped, or a neighbouring QWERTY key) away from a word with at
  least 1000 hits in the OpenSubtitles 2018 Indonesian word counts
  (hermitdave/FrequencyWords, `content/2018/id/id_full.txt`), and at least
  200 times rarer than it. Every candidate was reviewed by hand; real words
  in current use (`yakni`, `ketik`, `resi`, `pasi` as in *pucat pasi*,
  `utan` as in *orang utan*…) are kept in the script's `KEEP` list.
  `python3 -I tools/tighten_id.py apply` re-applies the list to a fresh
  upstream `id_ID.dic`.
