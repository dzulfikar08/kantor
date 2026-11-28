# 0006: PLS-SEM is a pure Rust engine inside Kantor

- **Status:** accepted (2026-11-27). Phase 1: the engine (`crates/kantor_pls`), `kantor pls`, the MCP tool `pls_run` and Paparan AI's `pls` tool. Phase 2 (the Sheets dialog, path diagram and results tab) builds on it.
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

**Front ends** (`crates/kantor`): `agent::pls` reads the data through the sheets engine (any format Kantor opens; CSV directly), runs the engine, makes the compact summary with rule-of-thumb flags, the APA-style `.docx` report (through the docs builder, English or Indonesian with decimal commas) and the results `.xlsx` (through the sheets builder). `kantor pls run|check|schema`, the MCP tool `pls_run` (no app needed; `check: true` only checks) and the agent harness's `pls` tool all call it.

**Goldens**: `tools/pls/gen_data.py` makes synthetic datasets; `tools/pls/goldens.R` estimates them with seminr 2.6.0 and cSEM 0.7.1 (10 cases: PLS with each weighting scheme, PLSc, listwise and mean replacement, formative blocks, mediation); `tools/pls/smartpls_refs.py` turns SmartPLS 4's own output for 14 textbook models (the openpls-engine validation supplement, Zenodo 10.5281/zenodo.20511533, CC BY 4.0) into `tests/smartpls.json`. CI downloads that supplement's datasets (they aren't copied into the repository).

## Consequences

- Deterministic estimates agree with seminr and cSEM to 10⁻⁶ (inner VIFs 10⁻⁵) and with SmartPLS's printed values to 0.0005 (it prints three decimals). Bootstrap results agree with seminr's within resampling error (SE within 15 %, 2.5 %/97.5 % percentiles within 0.4 SE at 2000 subsamples).
- Known differences, kept on purpose and listed in docs/pls-json.md: iteration counts may be one lower than SmartPLS's; f² differs from seminr's (which re-estimates the model); for PLSc, Fornell–Larcker correlations and inner VIFs use the consistent construct correlations (seminr uses construct scores; no SmartPLS PLSc reference was available); bootstrap random numbers differ; reliability and HTMT are left blank for composites as SmartPLS does.
- Not done yet: PLSpredict, IPMA, multigroup analysis (MICOM), moderation with interaction terms, higher-order constructs (`dimensions` is parsed and rejected as unsupported), FIMIX/POS segmentation, CB-SEM.
- The Sheets UI (Data → PLS-SEM…, a path diagram editor, a results tab) only needs the same `agent::pls` calls.

## Phase 2: the Sheets UI (2026-11-28)

- **A model tab in the sheet tab bar, not a new app tab.** A model belongs to the workbook its data is in: it is read from the workbook's cells at every Calculate (edits to the data count), and it is saved *in* the workbook (a custom XML part `https://kantor.paparanai.id/2026/pls` holding each model's pls-json, the scripts part's mechanism; Excel keeps it and ignores it). So the model shows like a chart sheet: a tab after the sheets (network icon) that replaces the grid with the model view, as SmartPLS keeps data and models in one project. A separate workspace tab would need a link back to an open workbook that can close or be renamed. Several models per workbook are allowed; Delete Model is on the tab's menu.
- **The model JSON is the file format.** `kantor_pls::Model` gained an optional `layout` (name, construct positions, data sheet and range, one- or two-tailed p values) that estimation ignores, so Save model as .json writes a file `kantor pls run` accepts as is, and Open model reads hand-written models (laid out automatically).
- **Drawing with GPUI paths, not `slides_scene`.** The diagram is ellipses, boxes and straight arrows with horizontal text; Slides' shape engine is for OOXML geometry and would bring EMU conversions and a scene cache for no gain. The canvas paints lines and polygons with `PathBuilder` and puts labels in plain absolutely positioned elements (GPUI has no rotated text, and none is needed). The editing operations, auto-layout (columns by longest path from a source), geometry and hit tests live in `sheets_ui::pls::diagram`, tested without a window.
- **Estimation off the UI thread.** `kantor_pls::estimate_with` takes a `Progress` (subsamples and jackknife rows done, and a cancel flag checked before each one); the tab polls it every 100 ms for the progress bar, and Stop cancels. Results are kept with the model they were estimated for; after an edit the diagram drops the numbers and the results view says to calculate again.
- **Reports through hooks.** The .docx report and the results .xlsx are built by the `kantor` crate (docs and sheets builders), so it registers them with Sheets as a GPUI global (`PlsHooks`); the result tables in the view, Copy table (tab-separated) and Insert as new sheet are made in `sheets_ui::pls::labels` with the same rule-of-thumb flags (loadings < 0.708, alpha/rho_A/rho_C < 0.70, rho_C > 0.95, AVE < 0.50, Fornell–Larcker, HTMT > 0.85, VIF ≥ 5, SRMR ≥ 0.08, p ≥ alpha).
- **Explain results** writes `model.json`, `data.csv` (the data estimated) and `results.json` (the summary `pls_run` returns) to a temporary folder and opens Paparan AI's chat with them attached and a prompt typed (a document job); the user presses Enter. The agent reads the files and can run its `pls` tool on them.
