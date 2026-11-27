#!/usr/bin/env python3
"""Turn the SmartPLS 4 reference values of the openpls-engine validation
supplement (Jacobsen 2026, Zenodo 10.5281/zenodo.20511533, CC BY 4.0) into
kantor_pls's external golden cases.

    curl -L -o supp.zip https://zenodo.org/api/records/20511533/files/openpls-validation-supplementary.zip/content
    unzip supp.zip -d supp
    python3 tools/pls/smartpls_refs.py supp/supplementary

Writes crates/kantor_pls/tests/smartpls.json: per case the model (in
Kantor's model JSON), the options SmartPLS ran with and the reference
values SmartPLS printed (three decimals). The datasets are not copied
into Kantor: the test reads them from $KANTOR_PLS_SMARTPLS (the unzipped
`supplementary` folder) and skips these cases without it.
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates" / "kantor_pls" / "tests" / "smartpls.json"

# model file -> (sidecar stem, dataset folder)
CASES = {
    "01-corporate-reputation-simple": ("corporate-reputation-primer__1-simple-model", "corporate-reputation"),
    "02-corporate-reputation-extended": ("corporate-reputation-primer__2-extended-model", "corporate-reputation"),
    "03a-redundancy-attr": ("corporate-reputation-primer__3a-redundancy-analysis-attr", "corporate-reputation"),
    "03b-redundancy-csor": ("corporate-reputation-primer__3b-redundancy-analysis-csor", "corporate-reputation"),
    "03c-redundancy-perf": ("corporate-reputation-primer__3c-redundancy-analysis-perf", "corporate-reputation"),
    "03d-redundancy-qual": ("corporate-reputation-primer__3d-redundancy-analysis-qual", "corporate-reputation"),
    "09-corporate-reputation-advanced": ("corporate-reputation-advanced__corporate-reputation-model", "corporate-reputation"),
    "10-ecsi": ("ecsi__pls-sem", "ecsi"),
    "11-acsi-modified": ("acsi-modified__pls-sem", "acsi"),
    "12-tam": ("tam__pls-sem", "tam"),
    "13-utaut": ("utaut__pls-sem", "utaut"),
    "14-oi": ("oi__pls-sem", "oi"),
    "15-oi-variations": ("oi-variations__organizational-identification", "oi-variations"),
    "18-employee-retention": ("employee-retention__mda-book-er-pls-sem", "employee-retention"),
}


def main(sup: Path):
    out = []
    for stem, (side, dataset) in CASES.items():
        m = json.loads((sup / "models" / f"{stem}.json").read_text())
        ref_path = sup / "sidecars" / f"{side}.reference.json"
        if not ref_path.exists():
            print(f"{stem}: no reference sidecar", file=sys.stderr)
            continue
        ref = json.loads(ref_path.read_text())
        ref.pop("_doc", None)
        model = {
            "constructs": [
                {"name": lv["name"], "indicators": lv["indicators"]}
                | ({"type": "composite"} if lv["mode"] == "B" else {})
                for lv in m["lvs"]
            ],
            "paths": [[p["source"], p["target"]] for p in m["paths"]],
        }
        sp = m.get("_smartpls", {})
        options = {
            "algorithm": "pls",
            "weighting": m["scheme"].lower(),
            "max_iterations": sp.get("max_iterations", 300),
        }
        if sp.get("missing_strategy") == "MEAN_REPLACEMENT":
            options |= {"missing": "mean", "missing_value": -99}
        else:
            options |= {"missing_value": -99}
        out.append({
            "name": stem,
            "title": m["name"],
            "dataset": f"datasets/{dataset}/data.txt",
            "model": model,
            "options": options,
            "reference": ref,
        })
    OUT.write_text(json.dumps({
        "_source": "SmartPLS 4 reference values from the openpls-engine validation supplement "
                   "(J. Jacobsen, 2026, https://doi.org/10.5281/zenodo.20511533, CC BY 4.0), "
                   "printed by SmartPLS to three decimals. Datasets: the same supplement "
                   "($KANTOR_PLS_SMARTPLS).",
        "cases": out,
    }, indent=1) + "\n")
    print(f"{OUT.relative_to(ROOT)}: {len(out)} cases", file=sys.stderr)


if __name__ == "__main__":
    main(Path(sys.argv[1]))
