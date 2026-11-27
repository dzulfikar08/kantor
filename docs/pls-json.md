# PLS-SEM from JSON: `kantor pls` and the MCP tool `pls_run`

Kantor estimates partial least squares structural equation models (PLS-SEM,
what SmartPLS does) on survey data in a workbook or CSV file, without a
window: PLS or consistent PLS (PLSc), the measurement model's reliability
and validity, the structural model's paths, R², f² and effects, model fit,
and bootstrap inference. The engine is the `kantor_pls` crate (no UI, no
file access; see [decision 0006](decisions/0006-pls-sem.md)).

```sh
kantor pls check --model model.json --data survey.xlsx           # the model against the data
kantor pls run   --model model.json --data survey.xlsx            # estimate, print a summary
kantor pls run   --model model.json --data survey.xlsx --sheet Data --range A1:X251 \
                 --bootstrap 5000 --seed 7 --out report.docx --out results.xlsx --lang id
kantor pls run   --model model.json --data survey.csv --json      # the summary as JSON
kantor pls schema > pls-json.schema.json
```

The MCP server (`kantor mcp`) offers `pls_check` and `pls_run` with the same
arguments (absolute paths), and Paparan AI's agent has a `pls` tool on the
files of its work folder. `pls_run` returns the summary below; its `report`,
`workbook` and `results_json` arguments write the files.

## The data

The first row of the range names the columns; each row below is one case
(respondent). Any format Kantor opens: `.xlsx`, `.xlsm`, `.xls`, `.xlsb`,
`.ods`, `.csv`, `.tsv`. Empty cells are missing values, and so is text in a
column (`check` counts it). A code such as `-99` is missing with
`missing_value`. Missing values are handled by **listwise deletion** (the
default: rows with a missing value in any indicator the model uses are left
out) or **mean replacement** (`missing: "mean"`, SmartPLS's default).

## The model

```json
{
  "constructs": [
    {"name": "IMAG", "indicators": ["IMAG1", "IMAG2", "IMAG3", "IMAG4", "IMAG5"]},
    {"name": "EXPE", "indicators": ["CUEX1", "CUEX2", "CUEX3"]},
    {"name": "QUAL", "indicators": ["qual_1", "qual_2", "qual_3"], "type": "composite"},
    {"name": "SAT",  "indicators": ["CUSA1", "CUSA2", "CUSA3"]},
    {"name": "LOY",  "indicators": ["CUSL1", "CUSL3"]}
  ],
  "paths": [
    {"from": "IMAG", "to": ["EXPE", "SAT", "LOY"]},
    ["EXPE", "SAT"],
    ["QUAL", "SAT"],
    ["SAT", "LOY"]
  ],
  "options": {"algorithm": "pls", "bootstrap": {"samples": 5000, "seed": 1}}
}
```

- `constructs`: `name`, `indicators` (column names; matched exactly, then
  ignoring case and outer spaces), `type` and `weights`:
  - `reflective` (default): a common factor measured by its indicators,
    Mode A weights; PLSc corrects it for measurement error; reliability,
    AVE and HTMT describe it.
  - `composite` (alias `formative`): a weighted sum of its indicators, Mode B
    (regression) weights unless `"weights": "mode_a"`; no reliability.
  - A single indicator is its construct (loading 1, reliability 1).
- `paths`: `["from", "to"]`, or `{"from": "A" | ["A", "B"], "to": "C" | [...]}`
  (every `from` to every `to`, like seminr's `paths()`). The structural model
  must be recursive (no cycles), and every construct needs a path.
- `options` (each may be overridden by a command-line option or a tool
  argument):

| Key | Default | Meaning |
|---|---|---|
| `algorithm` | `pls` | `pls`, or `plsc` (consistent PLS, Dijkstra & Henseler 2015) |
| `weighting` | `path` | inner weighting scheme: `path`, `centroid`, `factorial` |
| `max_iterations` | 300 | |
| `stop` | 1e-7 | stop when the outer weights change by less (sum of absolute changes) |
| `missing` | `listwise` | or `mean` |
| `missing_value` | none | a number meaning missing (`-99`) |
| `bootstrap` | none | `{samples: 5000, seed: 1, alpha: 0.05, bca: true, sign_changes: "none"}` |

`dimensions` and `higher_order` on a construct are reserved for
higher-order constructs (repeated indicators, two-stage), a later phase:
models with them are refused for now.

## What is estimated

- **Outer model**: loadings, weights, cross-loadings, outer VIF; per
  reflective construct Cronbach's alpha (standardized), rho_A
  (Dijkstra–Henseler), rho_C (composite reliability), AVE.
- **Discriminant validity**: Fornell–Larcker (√AVE on the diagonal), HTMT
  (absolute correlations, as SmartPLS 4) and HTMT2 (geometric means,
  Roemer et al. 2021); a single indicator's monotrait mean counts as 1, and
  composites have none.
- **Inner model**: path coefficients, R², adjusted R², f², inner VIF, BIC,
  total effects, total indirect effects and each specific indirect effect
  (every chain of paths).
- **Model fit** for the saturated and the estimated model: SRMR, d_ULS,
  d_G, chi² and NFI (below).
- **Bootstrap** (`samples` resamples of the rows, seeded, on every core):
  mean, standard error, t (original / SE), two-tailed p (Student t with
  samples − 1 degrees of freedom) and one-tailed p (half of it), the
  percentile interval and the bias-corrected and accelerated (BCa) interval
  (acceleration from a jackknife), for paths, loadings, weights, R², f²,
  total indirect, specific indirect and total effects, and HTMT (with the
  one-sided upper bound for the HTMT < 0.85/0.90 test). The same seed gives
  the same numbers on any machine and number of threads.

## Output

`kantor pls run` prints constructs, paths and fit as text (`--json`: the
summary). `--out` (repeatable) writes:

- `.docx`: an APA-style report in English or Indonesian (`--lang id`):
  estimation settings, a reliability and convergent validity table,
  formative indicators (weights, t, p, VIF), HTMT and Fornell–Larcker,
  path coefficients (β, SE, t, p, 95% CI, decision, f², VIF), R², specific
  indirect effects and model fit, each with an APA note giving the usual
  thresholds. Built with the docs JSON builder ([docs-json.md](docs-json.md)),
  so it is an ordinary editable Word file.
- `.xlsx`: a sheet per table (constructs, outer model, paths, HTMT, HTMT2,
  Fornell–Larcker, correlations, cross-loadings, effects, model fit), every
  bootstrap estimate, and the construct scores.
- `.json`: the full results (below), with the construct scores.

### The summary (`pls_run`, `--json`)

Compact, rounded to four decimals, for the AI to write the interpretation:

```json
{
  "n": 250, "rows_in_data": 250, "dropped_for_missing": 0, "replaced_missing": 0,
  "algorithm": "pls", "weighting": "path", "converged": true, "iterations": 7,
  "constructs": [{"name": "SAT", "type": "reflective", "indicators": 3,
                  "alpha": 0.779, "rho_a": 0.789, "rho_c": 0.871, "ave": 0.693,
                  "r2": 0.681, "r2_adj": 0.676, "r2_p": 0.0}],
  "outer": [{"construct": "SAT", "indicator": "CUSA1", "loading": 0.79, "vif": 1.5, "loading_p": 0.0}],
  "paths": [{"from": "IMAG", "to": "SAT", "beta": 0.179, "f2": 0.05, "vif": 1.9,
             "se": 0.05, "t": 3.6, "p": 0.0003, "ci": [0.08, 0.28], "ci_bca": [0.08, 0.27]}],
  "htmt": [{"a": "IMAG", "b": "SAT", "htmt": 0.83, "ci": [0.74, 0.91], "upper_one_sided": 0.9}],
  "specific_indirect": [{"path": ["IMAG", "EXPE", "SAT"], "effect": 0.03, "p": 0.2}],
  "total_effects": [{"from": "IMAG", "to": "LOY", "effect": 0.5, "p": 0.0}],
  "fit": {"srmr": 0.075, "d_uls": 1.69, "d_g": 0.65, "nfi": 0.69, "chi2": 918.1, "srmr_estimated": 0.08},
  "rule_of_thumb_flags": ["loading of CUSL2 is 0.548 (below 0.708)", "SRMR is 0.075 ..."],
  "warnings": [],
  "bootstrap": {"samples": 5000, "failed": 0, "seed": 1, "alpha": 0.05},
  "data": {"file": "/home/me/survey.xlsx", "sheet": "Data", "range": "A1:X251"}
}
```

`rule_of_thumb_flags` applies the usual guidelines (Hair et al. 2022):
loadings ≥ 0.708, rho_C between 0.70 and 0.95, AVE ≥ 0.50, Fornell–Larcker,
HTMT < 0.90, VIF < 5, SRMR < 0.08. They are guidelines for the text, not
verdicts.

### The full results (`.json`)

`kantor_pls::Results` serialised: `algorithm`, `weighting`, `n`, `rows`,
`dropped`, `replaced`, `iterations`, `converged`; `constructs` (`name`,
`type`, `weights`, `indicators`, `alpha`, `rho_a`, `rho_c`, `ave`, `r2`,
`r2_adj`, `bic`); `outer` (`construct`, `indicator`, `loading`, `weight`,
`vif`); `paths` (`from`, `to`, `coefficient`, `f2`, `vif`); tables
`correlations`, `fornell_larcker`, `htmt`, `htmt2`, `cross_loadings` as
`{rows, cols, values}` (null where undefined); `effects` (`total`,
`total_indirect`, `specific_indirect`); `fit` (`saturated`, `estimated`:
`srmr`, `d_uls`, `d_g`, `chi2`, `nfi`); `bootstrap` (`samples`, `failed`,
`not_converged`, `seed`, `alpha`, `sign_changes`, and lists of estimates
`paths`, `loadings`, `weights`, `r2`, `f2`, `total_indirect`,
`specific_indirect`, `total`, `htmt`, each with `original`, `mean`, `se`,
`t`, `p`, `p_one_tailed`, `ci`, `ci_bca`); `warnings`; `scores` (construct
scores, standardized, one row per case estimated).

## Conventions, and how they compare

Checked against seminr 2.6.0, cSEM 0.7.1 and SmartPLS 4 (the goldens of
`crates/kantor_pls/tests`, made by `tools/pls/`):

- **Estimates** (weights, loadings, paths, R², reliability, HTMT, VIFs,
  effects) agree with seminr and cSEM to 1e-6 and with every value SmartPLS 4
  printed for 14 textbook models (three decimals).
- **Model fit** follows SmartPLS 4: reflective constructs are common factors
  in the implied correlation matrix even in plain PLS (λλ' within a block),
  composites keep their observed block; SmartPLS's printed SRMR, d_ULS, d_G,
  chi² and NFI are the *saturated* model's. d_G uses base-10 logarithms as
  SmartPLS and cSEM do (Dijkstra & Henseler's paper writes ln). The
  estimated model's construct correlations follow cSEM.
- **f²** takes R² without the predictor from the same construct scores, as
  SmartPLS does; seminr re-estimates the model without the path, so its f²
  differs slightly.
- **rho_A, reliability and HTMT of composites** aren't reported (SmartPLS
  leaves them blank; seminr shows rho_A = 1 and alpha, rho_C, AVE).
- **Iterations** are counted from the first update; SmartPLS sometimes
  reports one more for the same stop criterion. The estimates don't differ.
- **Bootstrap** draws different random numbers from SmartPLS or seminr, so
  standard errors and intervals agree only within resampling error. Sign
  changes default to none (SmartPLS 4's default); `construct` and
  `individual` are there for older SmartPLS 3 habits. p values use Student's
  t with samples − 1 degrees of freedom.
