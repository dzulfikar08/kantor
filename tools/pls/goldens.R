# Reference results for kantor_pls's golden tests, from seminr and cSEM.
#
#   Rscript tools/pls/goldens.R [CASE...]
#
# Reads crates/kantor_pls/tests/cases.json (each case: a CSV under tests/data
# or, for "external": true, under $KANTOR_PLS_EXTERNAL; a model JSON under
# tests/models; options), estimates every case with seminr (PLS / PLSc,
# reliability, HTMT, VIFs, f2, effects, bootstrap) and cSEM (fit measures:
# SRMR, d_ULS, d_G, chi2, NFI, saturated and estimated; HTMT2), and writes
# tests/goldens/CASE.json. Only R packages are run here (GPL-3); no code of
# theirs is copied into Kantor.
#
# R with seminr, cSEM and jsonlite: tools/pls/README.md says how to get one
# (micromamba + CRAN into ~/.cache/pls-oracle).

suppressPackageStartupMessages({
  library(jsonlite)
  library(seminr)
  library(cSEM)
})

args <- commandArgs(trailingOnly = TRUE)
script_dir <- dirname(normalizePath(sub("--file=", "", grep("--file=", commandArgs(FALSE), value = TRUE))))
root <- normalizePath(file.path(script_dir, "..", ".."))
tests <- file.path(root, "crates", "kantor_pls", "tests")
cases <- fromJSON(file.path(tests, "cases.json"), simplifyVector = FALSE)
external <- Sys.getenv("KANTOR_PLS_EXTERNAL", "")

opt <- function(o, name, default) if (is.null(o[[name]])) default else o[[name]]

expand_paths <- function(paths) {
  out <- list()
  for (p in paths) {
    if (is.null(names(p))) {
      out[[length(out) + 1]] <- c(p[[1]], p[[2]])
    } else {
      for (f in unlist(p$from)) for (t in unlist(p$to)) out[[length(out) + 1]] <- c(f, t)
    }
  }
  out
}

pairs_list <- function(m, rows, cols, keep = function(r, c) TRUE) {
  out <- list()
  for (r in rows) for (c in cols) {
    if (keep(r, c) && !is.na(m[r, c])) out[[length(out) + 1]] <- list(from = r, to = c, value = m[r, c])
  }
  out
}

named <- function(v) as.list(setNames(as.numeric(v), names(v)))

run_case <- function(case) {
  o <- case$options
  data_dir <- if (isTRUE(case$external)) external else file.path(tests, "data")
  d <- read.csv(file.path(data_dir, case$data), check.names = FALSE)
  model <- fromJSON(file.path(tests, "models", case$model), simplifyVector = FALSE)
  mv <- opt(o, "missing_value", NULL)
  if (!is.null(mv)) d[d == mv] <- NA
  inds <- unlist(lapply(model$constructs, function(c) unlist(c$indicators)))
  d <- d[, inds, drop = FALSE]
  if (opt(o, "missing", "listwise") == "listwise") {
    d <- d[complete.cases(d), , drop = FALSE]
  } else {
    for (v in inds) d[is.na(d[[v]]), v] <- mean(d[[v]], na.rm = TRUE)
  }
  plsc <- opt(o, "algorithm", "pls") == "plsc"
  scheme <- opt(o, "weighting", "path")
  paths <- expand_paths(model$paths)
  cons <- model$constructs
  cname <- sapply(cons, function(c) c$name)
  kind <- sapply(cons, function(c) if (is.null(c$type)) "reflective" else c$type)
  wmode <- sapply(seq_along(cons), function(i) {
    w <- cons[[i]]$weights
    if (!is.null(w)) w else if (kind[i] == "composite") "mode_b" else "mode_a"
  })
  nind <- sapply(cons, function(c) length(c$indicators))
  factor <- plsc & kind == "reflective" & nind > 1

  out <- list(case = case$name, n = nrow(d),
              sources = list(seminr = as.character(packageVersion("seminr")),
                             cSEM = as.character(packageVersion("cSEM"))))

  # seminr: path and factorial weighting (no centroid).
  if (scheme != "centroid") {
    mm <- do.call(constructs, lapply(seq_along(cons), function(i) {
      items <- unlist(cons[[i]]$indicators)
      if (factor[i]) reflective(cname[i], items)
      else composite(cname[i], items, weights = if (wmode[i] == "mode_b") mode_B else mode_A)
    }))
    sm <- do.call(relationships, lapply(paths, function(p) paths(from = p[1], to = p[2])))
    m <- estimate_pls(d, mm, sm,
                      inner_weights = if (scheme == "factorial") path_factorial else path_weighting,
                      maxIt = 300, stopCriterion = 7)
    s <- summary(m)
    endo <- unique(sapply(paths, `[`, 2))
    pc <- m$path_coef
    seminr <- list(
      iterations = m$iterations,
      paths = lapply(paths, function(p) list(from = p[1], to = p[2], value = pc[p[1], p[2]])),
      loadings = as.list(setNames(sapply(seq_along(inds), function(i) {
        ci <- which(sapply(cons, function(c) inds[i] %in% unlist(c$indicators)))
        m$outer_loadings[inds[i], cname[ci]]
      }), inds)),
      weights = as.list(setNames(sapply(seq_along(inds), function(i) {
        ci <- which(sapply(cons, function(c) inds[i] %in% unlist(c$indicators)))
        m$outer_weights[inds[i], cname[ci]]
      }), inds)),
      r2 = as.list(setNames(m$rSquared["Rsq", endo], endo)),
      r2_adj = as.list(setNames(m$rSquared["AdjRsq", endo], endo)),
      reliability = lapply(setNames(cname, cname), function(c) as.list(s$reliability[c, ])),
      htmt = pairs_list(s$validity$htmt, cname, cname),
      fornell_larcker = pairs_list(s$validity$fl_criteria, cname, cname),
      cross_loadings = lapply(setNames(inds, inds), function(i) named(s$validity$cross_loadings[i, cname])),
      vif_items = as.list(unlist(unname(lapply(cname, function(c) {
        v <- s$validity$vif_items[[c]]
        if (length(v) == 0 || all(is.na(v))) NULL else setNames(as.numeric(v), names(v))
      })))),
      vif_antecedents = unlist(lapply(endo, function(e) {
        v <- s$vif_antecedents[[e]]
        if (length(v) < 2) return(NULL)
        lapply(names(v), function(f) list(from = f, to = e, value = unname(v[f])))
      }), recursive = FALSE),
      f2 = lapply(paths, function(p) list(from = p[1], to = p[2], value = s$fSquare[p[1], p[2]])),
      total = pairs_list(s$total_effects, cname, cname, function(r, c) s$total_effects[r, c] != 0),
      total_indirect = pairs_list(s$total_indirect_effects, cname, cname,
                                  function(r, c) s$total_indirect_effects[r, c] != 0)
    )
    boot_n <- opt(case, "bootstrap", 0)
    if (boot_n > 0) {
      b <- bootstrap_model(m, nboot = boot_n, seed = 123, cores = 4)
      sb <- summary(b)
      rows <- function(tab) lapply(rownames(tab), function(r) list(
        label = gsub("\\s+", " ", trimws(r)), original = tab[r, 1], mean = tab[r, 2], sd = tab[r, 3],
        lower = tab[r, 5], upper = tab[r, 6]))
      seminr$bootstrap <- list(samples = boot_n,
                               paths = rows(sb$bootstrapped_paths),
                               loadings = rows(sb$bootstrapped_loadings),
                               htmt = rows(sb$bootstrapped_HTMT))
    }
    out$seminr <- seminr
  }

  # cSEM: estimates under every scheme, and the fit measures.
  lines <- c()
  for (i in seq_along(cons)) {
    # Reflective blocks are common factors in the fit measures (SmartPLS 4's
    # convention); .disattenuate decides whether they are corrected (PLSc).
    op <- if (kind[i] == "reflective" && nind[i] > 1) "=~" else "<~"
    lines <- c(lines, paste(cname[i], op, paste(unlist(cons[[i]]$indicators), collapse = " + ")))
  }
  for (e in unique(sapply(paths, `[`, 2))) {
    preds <- sapply(Filter(function(p) p[2] == e, paths), `[`, 1)
    lines <- c(lines, paste(e, "~", paste(preds, collapse = " + ")))
  }
  modes <- list()
  for (i in seq_along(cons)) if (!(kind[i] == "reflective" && nind[i] > 1)) {
    modes[[cname[i]]] <- if (nind[i] == 1) "modeA" else if (wmode[i] == "mode_b") "modeB" else "modeA"
  }
  r <- csem(d, paste(lines, collapse = "\n"),
            .PLS_modes = if (length(modes)) modes else NULL,
            .PLS_weight_scheme_inner = scheme,
            .disattenuate = plsc,
            .iter_max = 300, .tolerance = 1e-7)
  est <- r$Estimates
  fit_of <- function(sat) list(
    srmr = calculateSRMR(r, .saturated = sat),
    d_uls = calculateDL(r, .saturated = sat),
    d_g = calculateDG(r, .saturated = sat),
    chi2 = calculateChiSquare(r, .saturated = sat),
    # calculateNFI() has no .saturated: (F0 - F_ML) / F0 with F0 = -log|S|.
    nfi = {
      S <- r$Estimates$Indicator_VCV
      f0 <- -log(det(S))
      (f0 - calculateDML(r, .saturated = sat)) / f0
    }
  )
  pe <- est$Path_estimates
  csem_out <- list(
    paths = lapply(paths, function(p) list(from = p[1], to = p[2], value = pe[p[2], p[1]])),
    loadings = as.list(setNames(sapply(seq_along(inds), function(i) {
      ci <- which(sapply(cons, function(c) inds[i] %in% unlist(c$indicators)))
      est$Loading_estimates[cname[ci], inds[i]]
    }), inds)),
    weights = as.list(setNames(sapply(seq_along(inds), function(i) {
      ci <- which(sapply(cons, function(c) inds[i] %in% unlist(c$indicators)))
      est$Weight_estimates[cname[ci], inds[i]]
    }), inds)),
    rho_a = as.list(est$Reliabilities),
    fit = list(saturated = fit_of(TRUE), estimated = fit_of(FALSE))
  )
  if (any(kind == "reflective" & nind > 1)) {
    # cSEM refuses HTMT2 (zeros) when a block's correlations mix signs.
    h2 <- suppressWarnings(tryCatch(calculateHTMT(r, .type_htmt = "htmt2", .absolute = TRUE)$htmts,
                                    error = function(e) NULL))
    if (!is.null(h2)) {
      csem_out$htmt2 <- pairs_list(h2, rownames(h2), colnames(h2), function(a, b) {
        match(a, rownames(h2)) > match(b, colnames(h2)) && h2[a, b] != 0
      })
    }
  }
  out$csem <- csem_out
  out
}

dir.create(file.path(tests, "goldens"), showWarnings = FALSE)
for (case in cases) {
  if (length(args) && !(case$name %in% args)) next
  if (isTRUE(case$external) && external == "") {
    message("skipping ", case$name, " (set KANTOR_PLS_EXTERNAL)")
    next
  }
  message("case ", case$name)
  g <- run_case(case)
  write(toJSON(g, auto_unbox = TRUE, digits = NA, pretty = TRUE, null = "null", na = "null"),
        file.path(tests, "goldens", paste0(case$name, ".json")))
}
