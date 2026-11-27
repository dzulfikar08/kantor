//! The standard normal and Student t distributions, for p values and BCa
//! intervals: erfc by W. J. Cody's rational approximations (1969), the
//! normal quantile by P. J. Acklam's algorithm polished with a Halley
//! step, and the t distribution through the regularized incomplete beta
//! function (continued fraction, modified Lentz).

use std::f64::consts::{FRAC_1_SQRT_2, PI};

/// erfc(x) for x ≥ 0 by Cody's rational approximations.
fn erfc_pos(x: f64) -> f64 {
    if x < 0.5 {
        return 1.0 - erf_small(x);
    }
    if x < 4.0 {
        const P: [f64; 9] = [
            5.641_884_969_886_701e-1,
            8.883_149_794_388_376,
            6.611_919_063_714_163e1,
            2.986_351_381_974_001e2,
            8.819_522_212_417_691e2,
            1.712_047_612_634_070_7e3,
            2.051_078_377_826_071_6e3,
            1.230_339_354_797_997_2e3,
            2.153_115_354_744_038_3e-8,
        ];
        const Q: [f64; 8] = [
            1.574_492_611_070_983_5e1,
            1.176_939_508_913_124_9e2,
            5.371_811_018_620_099e2,
            1.621_389_574_566_690_3e3,
            3.290_799_235_733_459_7e3,
            4.362_619_090_143_247e3,
            3.439_367_674_143_721_6e3,
            1.230_339_354_803_749_5e3,
        ];
        let mut num = P[8] * x;
        let mut den = x;
        for i in 0..7 {
            num = (num + P[i]) * x;
            den = (den + Q[i]) * x;
        }
        let r = (num + P[7]) / (den + Q[7]);
        return (-x * x).exp() * r;
    }
    const P: [f64; 6] = [
        3.053_266_349_612_323_4e-1,
        3.603_448_999_498_044_4e-1,
        1.257_817_261_112_292_6e-1,
        1.608_378_514_874_227_5e-2,
        6.587_491_615_298_378e-4,
        1.631_538_713_730_096_6e-2,
    ];
    const Q: [f64; 5] = [
        2.568_520_192_289_822,
        1.872_952_849_923_460_4,
        5.279_051_029_514_284e-1,
        6.051_834_131_244_132e-2,
        2.335_204_976_268_691_8e-3,
    ];
    let z = 1.0 / (x * x);
    let mut num = P[5] * z;
    let mut den = z;
    for i in 0..4 {
        num = (num + P[i]) * z;
        den = (den + Q[i]) * z;
    }
    let r = z * (num + P[4]) / (den + Q[4]);
    let r = (1.0 / PI.sqrt() - r) / x;
    (-x * x).exp() * r
}

/// erf(x) for |x| < 0.5.
fn erf_small(x: f64) -> f64 {
    const A: [f64; 5] = [
        3.161_123_743_870_565_6,
        1.138_641_541_510_501_6e2,
        3.774_852_376_853_020_2e2,
        3.209_377_589_138_469_5e3,
        1.857_777_061_846_031_5e-1,
    ];
    const B: [f64; 4] = [
        2.360_129_095_234_412e1,
        2.440_246_379_344_441_7e2,
        1.282_616_526_077_372_3e3,
        2.844_236_833_439_170_6e3,
    ];
    let z = x * x;
    let mut num = A[4] * z;
    let mut den = z;
    for i in 0..3 {
        num = (num + A[i]) * z;
        den = (den + B[i]) * z;
    }
    x * (num + A[3]) / (den + B[3])
}

fn erfc(x: f64) -> f64 {
    if x >= 0.0 {
        erfc_pos(x)
    } else {
        2.0 - erfc_pos(-x)
    }
}

/// Φ(x), the standard normal distribution function.
pub(crate) fn normal_cdf(x: f64) -> f64 {
    0.5 * erfc(-x * FRAC_1_SQRT_2)
}

/// Φ⁻¹(p); ±∞ at 0 and 1, NaN outside.
pub(crate) fn normal_quantile(p: f64) -> f64 {
    if !(0.0..=1.0).contains(&p) || p.is_nan() {
        return f64::NAN;
    }
    if p == 0.0 {
        return f64::NEG_INFINITY;
    }
    if p == 1.0 {
        return f64::INFINITY;
    }
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let lo = 0.024_25;
    let x = if p < lo {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - lo {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    // One Halley step brings it to full double precision.
    let e = normal_cdf(x) - p;
    let u = e * (2.0 * PI).sqrt() * (x * x / 2.0).exp();
    x - u / (1.0 + x * u / 2.0)
}

/// ln Γ(x) for x > 0 (Lanczos, g = 7, n = 9).
fn ln_gamma(x: f64) -> f64 {
    const G: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        return (PI / (PI * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = G[0];
    let t = x + 7.5;
    for (i, g) in G.iter().enumerate().skip(1) {
        a += g / (x + i as f64);
    }
    0.5 * (2.0 * PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// The continued fraction of the incomplete beta function (modified Lentz).
fn beta_cf(a: f64, b: f64, x: f64) -> f64 {
    const TINY: f64 = 1e-300;
    let mut c = 1.0;
    let mut d = 1.0 - (a + b) * x / (a + 1.0);
    if d.abs() < TINY {
        d = TINY;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=500 {
        let m = f64::from(m);
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((a + m2 - 1.0) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + aa / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m) * (a + b + m) * x / ((a + m2) * (a + m2 + 1.0));
        d = 1.0 + aa * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + aa / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-15 {
            break;
        }
    }
    h
}

/// The regularized incomplete beta function I_x(a, b).
fn beta_inc(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let ln_front = ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln();
    if x < (a + 1.0) / (a + b + 2.0) {
        ln_front.exp() * beta_cf(a, b, x) / a
    } else {
        1.0 - ln_front.exp() * beta_cf(b, a, 1.0 - x) / b
    }
}

/// Two-tailed p value of `t` with `df` degrees of freedom:
/// P(|T| ≥ |t|) = I_{df/(df+t²)}(df/2, 1/2).
pub(crate) fn t_two_tailed(t: f64, df: f64) -> f64 {
    if !t.is_finite() || !(df > 0.0) {
        return if t.is_infinite() { 0.0 } else { f64::NAN };
    }
    beta_inc(df / 2.0, 0.5, df / (df + t * t)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        assert!((normal_cdf(1.959_963_984_540_054) - 0.975).abs() < 1e-14);
        assert!((normal_cdf(-1.0) - 0.158_655_253_931_457_05).abs() < 1e-15);
        assert!((normal_cdf(5.0) - 0.999_999_713_348_428).abs() < 1e-14);
        for p in [1e-10, 0.001, 0.025, 0.3, 0.5, 0.9, 0.999] {
            assert!((normal_cdf(normal_quantile(p)) - p).abs() < 1e-14 * p.max(1e-3));
        }
        assert!((normal_quantile(0.975) - 1.959_963_984_540_054).abs() < 1e-12);
        // t = 2.0 with 10 df: two-tailed 0.073388...
        assert!((t_two_tailed(2.0, 10.0) - 0.073_388_034_770_740_7).abs() < 1e-10);
        assert!((t_two_tailed(1.96, 4999.0) - 0.050_052).abs() < 1e-5);
        assert!((t_two_tailed(0.0, 30.0) - 1.0).abs() < 1e-12);
    }
}
