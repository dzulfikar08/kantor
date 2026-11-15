//! The standard normal quantile as Excel computes it (NORM.S.INV): a
//! rational erfinv in the centre and Wichura's AS 241 tail forms, with the
//! same coefficients and order of operations, so that FORECAST.ETS.CONFINT
//! matches to the last bit. Ported from formulon's
//! `eval/stats/special_functions.cpp` (see `mod.rs`).

#![allow(clippy::excessive_precision, clippy::unreadable_literal)]

use super::num::flush;

const INV_CENTRAL_NUM: [f64; 8] = [
    -0.0004330290706835211,
    0.027356373384411086,
    -0.3322941145347177,
    1.6281100544846556,
    -3.9757965734067304,
    5.1435404286931545,
    -3.3766357514145087,
    0.886226925452758,
];
const INV_CENTRAL_DEN: [f64; 8] = [
    -0.002646479498649158,
    0.07626943316586525,
    -0.6700174948333386,
    2.7010353983086173,
    -5.758642552888369,
    6.7259595602369355,
    -4.0719248245651825,
    1.0,
];
const INV_MID_NUM: [f64; 8] = [
    0.25847255069830466,
    -2.3976335414463774,
    -3.0532108554060295,
    30.976557991052122,
    -39.367053299570436,
    5.408036523806584,
    7.990196417932504,
    0.275023361607254,
];
const INV_MID_DEN: [f64; 6] = [
    -13.421522241839597,
    47.829918125451506,
    -43.13381861299162,
    -6.567659888139633,
    14.39632192561636,
    1.0,
];
const INV_OUTER_NUM: [f64; 10] = [
    -2268.4679387671704,
    3792.9191227856427,
    7896.77800715066,
    -7373.827195352679,
    -2404.4850291650655,
    3986.6800664834323,
    -1516.411994362311,
    266.5928863464388,
    -22.671853278015895,
    0.7544123359505395,
];
const INV_OUTER_DEN: [f64; 10] = [
    -2326.6036307843597,
    830.4309565647657,
    11958.593279730147,
    -5599.514967112206,
    -7097.07852248484,
    6685.428862859329,
    -2257.3172698821236,
    374.84477038951684,
    -30.785006938269603,
    1.0,
];
const TAIL_NUM_NEAR: [f64; 8] = [
    0.0007745450142783414,
    0.022723844989269184,
    0.2417807251774506,
    1.2704582524523684,
    3.6478483247632045,
    5.769497221460691,
    4.630337846156546,
    1.4234371107496835,
];
const TAIL_DEN_NEAR: [f64; 8] = [
    1.0507500716444169e-09,
    0.0005475938084995345,
    0.015198666563616457,
    0.14810397642748008,
    0.6897673349851,
    1.6763848301838038,
    2.053191626637759,
    1.0,
];
const TAIL_NUM_FAR: [f64; 8] = [
    2.0103343992922881e-07,
    2.7115555687434876e-05,
    0.0012426609473880784,
    0.026532189526576124,
    0.29656057182850487,
    1.7848265399172913,
    5.463784911164114,
    6.657904643501103,
];
const TAIL_DEN_FAR: [f64; 8] = [
    2.0442631033899397e-15,
    1.421511758316446e-07,
    1.8463183175100548e-05,
    0.0007868691311456133,
    0.014875361290850615,
    0.1369298809227358,
    0.599832206555888,
    1.0,
];

fn horner(c: &[f64], y: f64) -> f64 {
    let mut acc = c[0];
    for &ci in &c[1..] {
        acc = flush(acc * y);
        acc += ci;
    }
    acc
}

/// Tail rational in w = sqrt(-ln q), q the smaller side; `a` is the lower
/// tail probability and `b` its complement.
fn tail_quantile(a: f64, b: f64) -> f64 {
    let upper = a > 0.5;
    let w = (-(if upper { b } else { a }).ln()).sqrt();
    let mut s = w - 5.0;
    let r = if s > 0.0 {
        flush(horner(&TAIL_NUM_FAR, s) / horner(&TAIL_DEN_FAR, s))
    } else {
        s = w - 1.6;
        flush(horner(&TAIL_NUM_NEAR, s) / horner(&TAIL_DEN_NEAR, s))
    };
    if upper {
        r
    } else {
        -r
    }
}

/// Inverse error function in three regions of |y|, tail form beyond 0.9375.
fn erfinv(y: f64) -> f64 {
    if y == 0.0 {
        return 0.0;
    }
    let v = y.abs();
    let t = flush(v * v);
    let r = if t <= 0.5 {
        flush(flush(horner(&INV_CENTRAL_NUM, t) / horner(&INV_CENTRAL_DEN, t)) * v)
    } else if v > 0.9375 {
        let a = flush((1.0 + v) * 0.5);
        let b = flush((1.0 - v) * 0.5);
        flush(tail_quantile(a, b) * 0.7071067811865476)
    } else {
        let w = (-(1.0 - v).ln()).sqrt();
        let r = if v > 0.85 {
            let u = v - 0.87890625;
            flush(horner(&INV_OUTER_NUM, u) / horner(&INV_OUTER_DEN, u))
        } else {
            flush(horner(&INV_MID_NUM, t) / horner(&INV_MID_DEN, t))
        };
        flush(r * w)
    };
    if y < 0.0 {
        -r
    } else {
        r
    }
}

/// z with Φ(z) = p, for 0 < p < 1.
pub(crate) fn normal_quantile(p: f64) -> f64 {
    if !(0.075..=0.9375).contains(&p) {
        return tail_quantile(p, 1.0 - p);
    }
    let y = flush(p * 2.0) - 1.0;
    flush(erfinv(y) * 1.4142135623730951)
}
