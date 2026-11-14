#!/usr/bin/env python3
"""Tighten the bundled Indonesian dictionary (see dict/README.md).

hunspell-id lists every headword of an old general dictionary, including
archaic, regional and non-standard spellings ("tingi", "masarakat",
"semejak", "jereka"). Many of them are one keystroke away from everyday
words, so the typo "tingi" (for "tinggi") passed. This script finds those
entries and drops them.

    candidates  python3 -I tighten_id.py candidates FREQ.txt > cands.tsv
    apply       python3 -I tighten_id.py apply

`candidates` lists entries W of dict/id_ID.dic that
  - have no affix flags (bare headwords: the rare ones), are lower case
    and at least 3 letters,
  - are one typical typo away from a headword V that is common in running
    text (>= 1000 hits in FREQ.txt, a "word count" list such as
    hermitdave/FrequencyWords content/2018/id/id_full.txt, OpenSubtitles):
    a dropped letter (not the first), a doubled letter typed once or a
    single letter typed twice, two neighbouring letters swapped, or one
    letter replaced by a neighbouring key on a QWERTY keyboard,
  - are themselves at least 200 times rarer than V in FREQ.txt.
Every candidate was then reviewed by hand: real words in current use are
listed in KEEP below; the rest went into dict/id_ID-removed.tsv. Words
that are part of a multi-word or hyphenated entry are always kept.

`apply` removes the words in dict/id_ID-removed.tsv from dict/id_ID.dic
(idempotent: it rewrites the count on the first line too).
"""
import sys
from pathlib import Path

DICT = Path(__file__).resolve().parent.parent / "dict"
DIC = DICT / "id_ID.dic"
REMOVED = DICT / "id_ID-removed.tsv"

# Candidates that are real words in current use (standard, technical,
# food, plants and animals, loanwords): kept.
KEEP = set("""
adas aki alu alay alel amil anal ayan baki bani basa beluk beruas bewok
biang bidara bika biku bilis bokeh botok bui burik butut buya coto cuka
cum dacing dai dam darma dini drat duku dus emak enau fan fikus fulan
gagu gambang gambut gang gepuk gilang hadis helah iota jemari jua juz
kadi kailan kak kakao kalam kalimah karkas karst kasa kasau kausa khat kit
koli konyak kua kulot kurs kutang langgang laun lema lesat lira lup luks
maag magi mami mangan maket maser mat melek meson midi mika minim
mustahik nadi nadir naib nila ode pagu paking pandan pari paria pasat
pasi pati payau pera perisa plang plano polusi porang posel pukang
puring pupa putu radis rak ranah resi reng rho riam runtut saf saga
sahaya saka saung secang sedan sehati seia sein selaku sepal sepat sia
sidat siri sin sintal sitasi soang spa suam surah tahi tampah tang
teknolog tembam tepa tifa timus toga umrah uni upas utan velar verso
via vide visa waham wedang yakni yakut zet ketik ketus
""".split())


def entries():
    lines = DIC.read_text(encoding="utf-8").splitlines()
    return lines[0], lines[1:]


def parts_of_compounds(lines):
    """Words that appear inside multi-word or hyphenated entries."""
    out = set()
    for line in lines:
        word = line.split("/", 1)[0]
        if " " in word or "-" in word:
            out.update(p.lower() for p in word.replace("-", " ").split())
    return out


ROWS = ["qwertyuiop", "asdfghjkl", "zxcvbnm"]
POS = {c: (r, i) for r, row in enumerate(ROWS) for i, c in enumerate(row)}


def neighbours(a, b):
    (r1, i1), (r2, i2) = POS[a], POS[b]
    return a != b and abs(r1 - r2) <= 1 and abs(i1 - i2) <= 1


def typos(v):
    out = {}
    for i in range(len(v)):
        if i + 1 < len(v) and v[i] == v[i + 1]:
            out[v[:i] + v[i + 1:]] = "undouble"
        if i > 0:
            out.setdefault(v[:i] + v[i + 1:], "drop")
        out.setdefault(v[:i] + v[i] + v[i:], "double")
        if i + 1 < len(v) and v[i] != v[i + 1]:
            out.setdefault(v[:i] + v[i + 1] + v[i] + v[i + 2:], "swap")
        for c in "abcdefghijklmnopqrstuvwxyz":
            if v[i] in POS and neighbours(v[i], c):
                out.setdefault(v[:i] + c + v[i + 1:], "key")
    out.pop(v, None)
    return out


def candidates(freq_path):
    freq = {}
    for line in open(freq_path, encoding="utf-8"):
        p = line.split()
        if len(p) == 2:
            freq[p[0]] = int(p[1])
    _, lines = entries()
    heads = {l.split("/", 1)[0] for l in lines}
    bare = {l for l in lines if "/" not in l and l.islower() and l.isalpha() and len(l) >= 3}
    found = {}
    for v, f in sorted(freq.items(), key=lambda x: -x[1]):
        if f < 1000 or not v.isalpha() or v not in heads:
            continue
        for w, kind in typos(v).items():
            if w in bare and freq.get(w, 0) * 200 < f and w not in found:
                found[w] = (v, kind)
    compound = parts_of_compounds(lines)
    for w in sorted(found):
        if w not in KEEP and w not in compound:
            print(f"{w}\t{found[w][0]}\t{found[w][1]}")


def apply():
    removed = set()
    for line in REMOVED.read_text(encoding="utf-8").splitlines():
        if line and not line.startswith("#"):
            removed.add(line.split("\t", 1)[0])
    _, lines = entries()
    kept = [l for l in lines if l.split("/", 1)[0] not in removed]
    DIC.write_text(f"{len(kept)}\n" + "\n".join(kept) + "\n", encoding="utf-8")
    print(f"removed {len(lines) - len(kept)} entries, {len(kept)} left", file=sys.stderr)


if __name__ == "__main__":
    if sys.argv[1:2] == ["candidates"] and len(sys.argv) == 3:
        candidates(sys.argv[2])
    elif sys.argv[1:] == ["apply"]:
        apply()
    else:
        sys.exit(__doc__)
