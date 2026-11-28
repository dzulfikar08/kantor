//! The public API: model JSON, errors, checks, bootstrap reproducibility.

use std::path::Path;

use kantor_pls::{
    Algorithm, Bootstrap, Construct, Data, Error, Layout, Level, Model, Options, Path as PathSpec,
    Progress, SignChanges, WeightMode, check, estimate, estimate_with,
};

fn data(name: &str) -> Data {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name);
    let text = std::fs::read_to_string(path).unwrap();
    let mut lines = text.lines();
    let names = lines
        .next()
        .unwrap()
        .split(',')
        .map(str::to_owned)
        .collect();
    let rows = lines
        .map(|l| {
            l.split(',')
                .map(|v| v.parse().unwrap_or(f64::NAN))
                .collect()
        })
        .collect();
    Data::new(names, rows).unwrap()
}

fn mediation() -> Model {
    let items = |c: &str| -> Vec<String> { (1..=4).map(|i| format!("{c}_{i}")).collect() };
    Model {
        constructs: ["X", "M", "Y"]
            .iter()
            .map(|c| Construct {
                name: (*c).to_owned(),
                indicators: items(c),
                ..Default::default()
            })
            .collect(),
        paths: vec![
            PathSpec::new("X", "M"),
            PathSpec::new("M", "Y"),
            PathSpec::new("X", "Y"),
        ],
        options: None,
        layout: None,
    }
}

#[test]
fn model_json_forms() {
    let m: Model = serde_json::from_str(
        r#"{
        "constructs": [
            {"name": "A", "indicators": ["a1", "a2"]},
            {"name": "B", "indicators": ["b1", "b2"], "type": "formative"},
            {"name": "C", "indicators": ["c1"], "type": "composite", "weights": "mode_a"},
            {"name": "D", "indicators": ["d1", "d2"]}
        ],
        "paths": [["A", "C"], {"from": ["A", "B"], "to": ["D"]}, {"from": "C", "to": "D"}],
        "options": {"algorithm": "plsc", "bootstrap": {"samples": 100}}
    }"#,
    )
    .unwrap();
    assert_eq!(m.paths.len(), 4);
    assert_eq!(m.paths[1], PathSpec::new("A", "D"));
    assert_eq!(m.constructs[1].weight_mode(), WeightMode::ModeB);
    assert_eq!(m.constructs[2].weight_mode(), WeightMode::ModeA);
    let o = m.options.unwrap();
    assert_eq!(o.algorithm, Algorithm::Plsc);
    assert_eq!(o.bootstrap.unwrap().samples, 100);
    assert!(serde_json::from_str::<Model>(r#"{"constructs": [], "bogus": 1}"#).is_err());
}

#[test]
fn errors_name_the_problem() {
    let d = data("mediation.csv");
    let mut m = mediation();
    m.paths.push(PathSpec::new("Y", "X"));
    let e = estimate(&d, &m, &Options::default())
        .unwrap_err()
        .to_string();
    assert!(e.contains("non-recursive"), "{e}");
    let mut m = mediation();
    m.constructs[0].indicators.push("nope".into());
    let e = estimate(&d, &m, &Options::default())
        .unwrap_err()
        .to_string();
    assert!(e.contains("nope"), "{e}");
    let mut m = mediation();
    m.constructs[2].dimensions = vec!["X".into()];
    assert!(matches!(
        estimate(&d, &m, &Options::default()),
        Err(kantor_pls::Error::Unsupported(_))
    ));
}

#[test]
fn check_finds_problems() {
    let mut d = data("mediation.csv");
    let model = mediation();
    let f = check(&d, &model, &Options::default());
    assert!(f.iter().all(|f| f.level != Level::Error), "{f:?}");
    assert!(f.iter().any(|f| f.check == "missing_values"));
    assert!(f.iter().any(|f| f.check == "listwise_deletion"));
    // A constant column, a missing column and few rows.
    let names = d.names().to_vec();
    let rows: Vec<Vec<f64>> = (0..12)
        .map(|r| {
            (0..names.len())
                .map(|c| if c == 0 { 3.0 } else { d.get(r, c) })
                .collect()
        })
        .collect();
    d = Data::new(names, rows).unwrap();
    let f = check(&d, &model, &Options::default());
    assert!(f.iter().any(|f| f.check == "constant_indicator"), "{f:?}");
    assert!(f.iter().any(|f| f.check == "ten_times_rule"), "{f:?}");
    let mut bad = mediation();
    bad.constructs[1].indicators[0] = "M_9".into();
    let f = check(&d, &bad, &Options::default());
    assert_eq!(f[0].check, "missing_column");
    assert_eq!(f[0].indicator.as_deref(), Some("M_9"));
}

#[test]
fn bootstrap_is_reproducible_and_complete() {
    let d = data("mediation.csv");
    let opts = Options {
        bootstrap: Some(Bootstrap {
            samples: 300,
            seed: 42,
            ..Default::default()
        }),
        ..Default::default()
    };
    let a = estimate(&d, &mediation(), &opts).unwrap();
    let b = estimate(&d, &mediation(), &opts).unwrap();
    assert_eq!(a.bootstrap, b.bootstrap);
    let boot = a.bootstrap.as_ref().unwrap();
    assert_eq!(boot.paths.len(), 3);
    assert_eq!(boot.specific_indirect.len(), 1);
    assert_eq!(
        boot.specific_indirect[0].path.as_deref().unwrap(),
        ["X", "M", "Y"]
    );
    assert_eq!(boot.total_indirect.len(), 1);
    assert_eq!(boot.htmt.len(), 3);
    for e in boot.paths.iter().chain(&boot.loadings) {
        let (lo, hi) = (e.ci[0].unwrap(), e.ci[1].unwrap());
        assert!(lo <= hi);
        let bca = e.ci_bca.unwrap();
        assert!(bca[0].unwrap() <= bca[1].unwrap());
        assert!(e.p.unwrap() >= 0.0 && e.p.unwrap() <= 1.0);
    }
    // X -> M is strong: significant.
    assert!(boot.paths[0].p.unwrap() < 0.001);
    let mut other = opts.clone();
    other.bootstrap.as_mut().unwrap().seed = 43;
    let c = estimate(&d, &mediation(), &other).unwrap();
    assert_ne!(a.bootstrap, c.bootstrap);
    for signs in [SignChanges::Construct, SignChanges::Individual] {
        let mut o = opts.clone();
        o.bootstrap.as_mut().unwrap().sign_changes = signs;
        o.bootstrap.as_mut().unwrap().bca = false;
        let r = estimate(&d, &mediation(), &o).unwrap();
        assert_eq!(r.bootstrap.unwrap().failed, 0);
    }
}

#[test]
fn bootstrap_reports_progress_and_stops() {
    let d = data("mediation.csv");
    let opts = Options {
        bootstrap: Some(Bootstrap {
            samples: 200,
            seed: 42,
            ..Default::default()
        }),
        ..Default::default()
    };
    let progress = Progress::new();
    let r = estimate_with(&d, &mediation(), &opts, Some(&progress)).unwrap();
    // 200 subsamples and a jackknife of the complete rows for BCa.
    let (done, total) = progress.get();
    assert_eq!(done, total);
    assert_eq!(total, 200 + r.n);
    // The same numbers as without progress.
    assert_eq!(
        r.bootstrap,
        estimate(&d, &mediation(), &opts).unwrap().bootstrap
    );
    let stopped = Progress::new();
    stopped.cancel();
    assert_eq!(
        estimate_with(&d, &mediation(), &opts, Some(&stopped)).unwrap_err(),
        Error::Cancelled
    );
    // Without a bootstrap there is nothing to stop.
    assert!(estimate_with(&d, &mediation(), &Options::default(), Some(&stopped)).is_ok());
}

#[test]
fn layout_round_trips_and_is_ignored() {
    let mut m = mediation();
    m.layout = Some(Layout {
        name: Some("Mediasi".into()),
        positions: [("X".to_owned(), [10.0, 20.0])].into_iter().collect(),
        sheet: Some("Data".into()),
        range: Some("A1:L151".into()),
        one_tailed: true,
    });
    let text = serde_json::to_string(&m).unwrap();
    let back: Model = serde_json::from_str(&text).unwrap();
    assert_eq!(back, m);
    let d = data("mediation.csv");
    let with = estimate(&d, &m, &Options::default()).unwrap();
    let without = estimate(&d, &mediation(), &Options::default()).unwrap();
    assert_eq!(with.paths, without.paths);
    // Unknown layout keys are refused like any other.
    let bad = text.replace("\"one_tailed\"", "\"two_tailed\"");
    assert!(serde_json::from_str::<Model>(&bad).is_err());
}

#[test]
fn scores_are_standardized_and_results_serialize() {
    let d = data("ecsi_synth.csv");
    let model: Model = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/ecsi.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let r = estimate(&d, &model, &Options::default()).unwrap();
    assert!(r.converged);
    assert_eq!(r.scores.values.len(), 250);
    for j in 0..r.scores.cols.len() {
        let col: Vec<f64> = r.scores.values.iter().map(|row| row[j].unwrap()).collect();
        let mean = col.iter().sum::<f64>() / col.len() as f64;
        let var = col.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (col.len() - 1) as f64;
        assert!(
            mean.abs() < 1e-9 && (var - 1.0).abs() < 1e-9,
            "{mean} {var}"
        );
    }
    let v = serde_json::to_value(&r).unwrap();
    assert!(v.get("scores").is_none());
    assert_eq!(v["constructs"][0]["type"], "reflective");
    assert!(v["fit"]["saturated"]["srmr"].as_f64().unwrap() > 0.0);
    assert_eq!(v["effects"]["specific_indirect"][0]["path"][0], "IMAG");
    let plsc = estimate(
        &d,
        &model,
        &Options {
            algorithm: Algorithm::Plsc,
            ..Default::default()
        },
    )
    .unwrap();
    // PLSc corrects a path between reflective constructs for attenuation.
    let a = r.path("EXPE", "QUAL").unwrap().coefficient.unwrap();
    let b = plsc.path("EXPE", "QUAL").unwrap().coefficient.unwrap();
    assert!(b > a, "{a} {b}");
}
