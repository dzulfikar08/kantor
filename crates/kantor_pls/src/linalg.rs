//! Small dense matrices: PLS-SEM works on correlation matrices of a few
//! dozen indicators, so a row-major `Vec<f64>` with LU, Cholesky and a
//! Jacobi eigen solver is all it needs (and keeps the crate free of
//! linear-algebra dependencies, for WASM).

use std::ops::{Index, IndexMut};

#[derive(Clone, Debug, PartialEq)]
pub struct Mat {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f64>,
}

impl Index<(usize, usize)> for Mat {
    type Output = f64;
    fn index(&self, (r, c): (usize, usize)) -> &f64 {
        &self.data[r * self.cols + c]
    }
}

impl IndexMut<(usize, usize)> for Mat {
    fn index_mut(&mut self, (r, c): (usize, usize)) -> &mut f64 {
        &mut self.data[r * self.cols + c]
    }
}

impl Mat {
    pub fn zeros(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            data: vec![0.0; rows * cols],
        }
    }

    pub fn identity(n: usize) -> Self {
        let mut m = Self::zeros(n, n);
        for i in 0..n {
            m[(i, i)] = 1.0;
        }
        m
    }

    pub fn from_fn(rows: usize, cols: usize, f: impl Fn(usize, usize) -> f64) -> Self {
        let mut m = Self::zeros(rows, cols);
        for r in 0..rows {
            for c in 0..cols {
                m[(r, c)] = f(r, c);
            }
        }
        m
    }

    pub fn row(&self, r: usize) -> &[f64] {
        &self.data[r * self.cols..(r + 1) * self.cols]
    }

    pub fn t(&self) -> Mat {
        Mat::from_fn(self.cols, self.rows, |r, c| self[(c, r)])
    }

    pub fn mul(&self, o: &Mat) -> Mat {
        assert_eq!(self.cols, o.rows, "matrix sizes");
        let mut m = Mat::zeros(self.rows, o.cols);
        for i in 0..self.rows {
            for k in 0..self.cols {
                let a = self[(i, k)];
                if a == 0.0 {
                    continue;
                }
                let orow = o.row(k);
                let mrow = &mut m.data[i * o.cols..(i + 1) * o.cols];
                for (x, b) in mrow.iter_mut().zip(orow) {
                    *x += a * b;
                }
            }
        }
        m
    }

    #[cfg(test)]
    pub fn sub(&self, o: &Mat) -> Mat {
        Mat {
            rows: self.rows,
            cols: self.cols,
            data: self.data.iter().zip(&o.data).map(|(a, b)| a - b).collect(),
        }
    }

    /// The submatrix of rows `rs` and columns `cs`.
    pub fn select(&self, rs: &[usize], cs: &[usize]) -> Mat {
        Mat::from_fn(rs.len(), cs.len(), |r, c| self[(rs[r], cs[c])])
    }

    /// `A x = b` by LU with partial pivoting (square `A`); `None` when
    /// singular.
    pub fn solve(&self, b: &Mat) -> Option<Mat> {
        let lu = Lu::new(self)?;
        Some(lu.solve(b))
    }

    pub fn inverse(&self) -> Option<Mat> {
        self.solve(&Mat::identity(self.rows))
    }

    /// `ln |det A|` and the sign of the determinant.
    pub fn log_det(&self) -> Option<(f64, f64)> {
        let lu = Lu::new(self)?;
        let mut ld = 0.0;
        let mut sign = lu.sign;
        for i in 0..self.rows {
            let d = lu.a[(i, i)];
            if d < 0.0 {
                sign = -sign;
            }
            ld += d.abs().ln();
        }
        Some((ld, sign))
    }

    /// Lower Cholesky factor of a symmetric positive definite matrix.
    pub fn cholesky(&self) -> Option<Mat> {
        let n = self.rows;
        let mut l = Mat::zeros(n, n);
        for j in 0..n {
            let mut d = self[(j, j)];
            for k in 0..j {
                d -= l[(j, k)] * l[(j, k)];
            }
            if d <= 0.0 || !d.is_finite() {
                return None;
            }
            let d = d.sqrt();
            l[(j, j)] = d;
            for i in j + 1..n {
                let mut s = self[(i, j)];
                for k in 0..j {
                    s -= l[(i, k)] * l[(j, k)];
                }
                l[(i, j)] = s / d;
            }
        }
        Some(l)
    }

    /// Eigenvalues of a symmetric matrix (cyclic Jacobi rotations).
    pub fn sym_eigenvalues(&self) -> Vec<f64> {
        let n = self.rows;
        let mut a = self.clone();
        for _sweep in 0..100 {
            let mut off = 0.0;
            for p in 0..n {
                for q in p + 1..n {
                    off += a[(p, q)] * a[(p, q)];
                }
            }
            if off < 1e-30 {
                break;
            }
            for p in 0..n {
                for q in p + 1..n {
                    let apq = a[(p, q)];
                    if apq.abs() < 1e-300 {
                        continue;
                    }
                    let theta = (a[(q, q)] - a[(p, p)]) / (2.0 * apq);
                    let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                    let t = if theta == 0.0 { 1.0 } else { t };
                    let c = 1.0 / (t * t + 1.0).sqrt();
                    let s = t * c;
                    for k in 0..n {
                        let akp = a[(k, p)];
                        let akq = a[(k, q)];
                        a[(k, p)] = c * akp - s * akq;
                        a[(k, q)] = s * akp + c * akq;
                    }
                    for k in 0..n {
                        let apk = a[(p, k)];
                        let aqk = a[(q, k)];
                        a[(p, k)] = c * apk - s * aqk;
                        a[(q, k)] = s * apk + c * aqk;
                    }
                }
            }
        }
        (0..n).map(|i| a[(i, i)]).collect()
    }
}

/// LU decomposition with partial pivoting.
struct Lu {
    a: Mat,
    perm: Vec<usize>,
    sign: f64,
}

impl Lu {
    fn new(m: &Mat) -> Option<Lu> {
        assert_eq!(m.rows, m.cols, "LU of a square matrix");
        let n = m.rows;
        let mut a = m.clone();
        let mut perm: Vec<usize> = (0..n).collect();
        let mut sign = 1.0;
        let scale = m
            .data
            .iter()
            .fold(0.0f64, |s, x| s.max(x.abs()))
            .max(1e-300);
        for k in 0..n {
            let (p, big) = (k..n)
                .map(|i| (i, a[(i, k)].abs()))
                .fold((k, -1.0), |b, x| if x.1 > b.1 { x } else { b });
            if big <= 1e-13 * scale || !big.is_finite() {
                return None;
            }
            if p != k {
                for c in 0..n {
                    a.data.swap(k * n + c, p * n + c);
                }
                perm.swap(k, p);
                sign = -sign;
            }
            let d = a[(k, k)];
            for i in k + 1..n {
                let f = a[(i, k)] / d;
                a[(i, k)] = f;
                if f != 0.0 {
                    for c in k + 1..n {
                        let v = a[(k, c)];
                        a[(i, c)] -= f * v;
                    }
                }
            }
        }
        Some(Lu { a, perm, sign })
    }

    fn solve(&self, b: &Mat) -> Mat {
        let n = self.a.rows;
        let mut x = Mat::zeros(n, b.cols);
        for col in 0..b.cols {
            let mut y: Vec<f64> = (0..n).map(|i| b[(self.perm[i], col)]).collect();
            for i in 0..n {
                let mut s = y[i];
                for k in 0..i {
                    s -= self.a[(i, k)] * y[k];
                }
                y[i] = s;
            }
            for i in (0..n).rev() {
                let mut s = y[i];
                for k in i + 1..n {
                    s -= self.a[(i, k)] * y[k];
                }
                y[i] = s / self.a[(i, i)];
            }
            for i in 0..n {
                x[(i, col)] = y[i];
            }
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solve_inverse_and_eigen() {
        let a = Mat {
            rows: 3,
            cols: 3,
            data: vec![4.0, 1.0, 0.5, 1.0, 3.0, 0.2, 0.5, 0.2, 2.0],
        };
        let inv = a.inverse().unwrap();
        let id = a.mul(&inv);
        for i in 0..3 {
            for j in 0..3 {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((id[(i, j)] - want).abs() < 1e-12);
            }
        }
        let ev = a.sym_eigenvalues();
        let trace: f64 = ev.iter().sum();
        assert!((trace - 9.0).abs() < 1e-10);
        let (ld, s) = a.log_det().unwrap();
        let prod: f64 = ev.iter().product();
        assert!((ld.exp() * s - prod).abs() < 1e-9);
        let l = a.cholesky().unwrap();
        let back = l.mul(&l.t());
        assert!(back.sub(&a).data.iter().all(|x| x.abs() < 1e-12));
        assert!(Mat::zeros(2, 2).inverse().is_none());
    }
}
