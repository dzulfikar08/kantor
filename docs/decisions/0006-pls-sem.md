# 0006: PLS-SEM is a pure Rust engine inside Kantor

- **Status:** accepted (2026-11-27). Phase 1: the engine (`crates/kantor_pls`), `kantor pls`, the MCP tools `pls_check` / `pls_run` and Paparan AI's `pls` tool. Phase 2 (the Sheets dialog, path diagram and results tab) builds on it.
- **Decides:** how Kantor estimates partial least squares structural equation models, the analysis Indonesian thesis students run in SmartPLS.

## Context

Most Indonesian management, accounting and education theses analyse questionnaires with PLS-SEM, almost always in SmartPLS (paid, and the free tier stops at 100 cases). Kantor already opens the survey workbook and writes the thesis; running the model in the same app, with a report in Indonesian, removes a paid tool from the chain. Results have to match SmartPLS closely enough that a supervisor comparing numbers sees no difference.

## Options

- **Call R (seminr, cSEM) or Python (plspm)**: needs a runtime Kantor doesn't ship; seminr and cSEM are GPL-3, so their code can't be taken over either.
- **Port an existing engine** (openpls-engine, plspm): licences (GPL) or languages that don't fit.
- **Write the engine from the published methods** (chosen): Lohmöller's PLS path algorithm, Dijkstra & Henseler's PLSc and rho_A, Henseler et al.'s HTMT, Roemer et al.'s HTMT2, Efron's BCa, Hair et al.'s primer for the reporting conventions. GPL tools are only *run* to make reference values.

## Decision

**Engine** (`kantor_pls`): no UI, no file access, no dependencies beyond serde, rand_chacha and (optional, on by default) rayon, so it can build for WASM later. Its own small dense linear algebra (`linalg`: Cholesky, solve, inverse, log-determinant, symmetric eigenvalues) and distributions (`dist`: normal and Student's t); the models are small (tens of indicators), so nothing needs BLAS.

- `Data` (n × p with names; missing values as NaN or a code; listwise deletion or mean replacement), `Model` (constructs reflective or composite, Mode A or B weights, paths; JSON in [docs/pls-json.md](../pls-json.md)), `Options`, `estimate` → `Results` (serialisable).
- PLS with path, centroid or factorial weighting (stop at 10⁻⁷, 300 iterations), or PLSc. Outer loadings and weights, alpha, rho_A, rho_C, AVE, Fornell–Larcker, HTMT and HTMT2, cross-loadings, outer and inner VIFs, paths, R² and adjusted R², f², specific and total indirect and total effects, SRMR, d_ULS, d_G, chi², NFI (saturated and estimated), BIC.
- Bootstrap: seeded (one ChaCha stream per subsample from the seed, so results don't depend on the thread count), run with rayon; 5000 subsamples by default; SE, t, p (Student's t, samples − 1 df), percentile and BCa intervals; sign-change options as SmartPLS 3.
- `check`: the model against the data before estimating (unknown columns, constant or text columns, missing values, sample size, collinear Mode B blocks, non-recursive models).

**Front ends** (`crates/kantor`): `agent::pls` reads the data through the sheets engine (any format Kantor opens; CSV directly), runs the engine, makes the compact summary with rule-of-thumb flags, the APA-style `.docx` report (through the docs builder, English or Indonesian with decimal commas) and the results `.xlsx` (through the sheets builder). `kantor pls run|check|schema`, the MCP tools `pls_check` and `pls_run` (no app needed) and the agent harness's `pls` tool all call it.

**Goldens**: `tools/pls/gen_data.py` makes synthetic datasets; `tools/pls/goldens.R` estimates them with seminr 2.6.0 and cSEM 0.7.1 (10 cases: PLS with each weighting scheme, PLSc, listwise and mean replacement, formative blocks, mediation); `tools/pls/smartpls_refs.py` turns SmartPLS 4's own output for 14 textbook models (the openpls-engine validation supplement, Zenodo 10.5281/zenodo.20511533, CC BY 4.0) into `tests/smartpls.json`. CI downloads that supplement's datasets (they aren't copied into the repository).

## Consequences

- Deterministic estimates agree with seminr and cSEM to 10⁻⁶ (inner VIFs 10⁻⁵) and with SmartPLS's printed values to 0.0005 (it prints three decimals). Bootstrap results agree with seminr's within resampling error (SE within 15 %, interval bounds within a quarter SE).
- Known differences from SmartPLS, kept on purpose and listed in docs/pls-json.md: iteration counts may be one lower; f² against seminr (which re-estimates); bootstrap random numbers; composite reliability and HTMT are left blank for composites as SmartPLS does.
- Not done yet: PLSpredict, IPMA, multigroup analysis (MICOM), moderation with interaction terms, higher-order constructs (`dimensions` is parsed and rejected as unsupported), FIMIX/POS segmentation, CB-SEM.
- The Sheets UI (Data → PLS-SEM…, a path diagram editor, a results tab) only needs the same `agent::pls` calls.
