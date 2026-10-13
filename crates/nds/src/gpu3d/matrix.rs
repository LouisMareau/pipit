//! 4 × 4 matrices in the geometry engine's 20.12 fixed point, kept and
//! multiplied the way the hardware does: row vectors, so a vertex is
//! `v × M`, and `MTX_MULT` makes the current matrix `New × Current`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Matrix(pub [i32; 16]);

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix =
        Matrix([0x1000, 0, 0, 0, 0, 0x1000, 0, 0, 0, 0, 0x1000, 0, 0, 0, 0, 0x1000]);

    /// `self = other × self`.
    pub fn mul_left(&mut self, other: &Matrix) {
        let cur = self.0;
        let o = &other.0;
        for row in 0..4 {
            for col in 0..4 {
                let mut acc: i64 = 0;
                for k in 0..4 {
                    acc += i64::from(o[row * 4 + k]) * i64::from(cur[k * 4 + col]);
                }
                self.0[row * 4 + col] = (acc >> 12) as i32;
            }
        }
    }

    /// `self = self × other`.
    pub fn mul_right(&mut self, other: &Matrix) {
        let cur = self.0;
        let o = &other.0;
        for row in 0..4 {
            for col in 0..4 {
                let mut acc: i64 = 0;
                for k in 0..4 {
                    acc += i64::from(cur[row * 4 + k]) * i64::from(o[k * 4 + col]);
                }
                self.0[row * 4 + col] = (acc >> 12) as i32;
            }
        }
    }

    /// The 16 parameters of MTX_LOAD_4x4 / MTX_MULT_4x4, row by row.
    pub fn from_4x4(p: &[u32]) -> Matrix {
        let mut m = [0i32; 16];
        for (i, v) in p.iter().take(16).enumerate() {
            m[i] = *v as i32;
        }
        Matrix(m)
    }

    /// The 12 parameters of the 4 × 3 forms: the fourth column is (0, 0, 0, 1).
    pub fn from_4x3(p: &[u32]) -> Matrix {
        let mut m = Matrix::IDENTITY;
        for row in 0..4 {
            for col in 0..3 {
                m.0[row * 4 + col] = p[row * 3 + col] as i32;
            }
            m.0[row * 4 + 3] = 0;
        }
        m.0[15] = 0x1000;
        m
    }

    /// The 9 parameters of MTX_MULT_3x3.
    pub fn from_3x3(p: &[u32]) -> Matrix {
        let mut m = Matrix::IDENTITY;
        for row in 0..3 {
            for col in 0..3 {
                m.0[row * 4 + col] = p[row * 3 + col] as i32;
            }
        }
        m
    }

    pub fn scale(x: i32, y: i32, z: i32) -> Matrix {
        let mut m = Matrix::IDENTITY;
        m.0[0] = x;
        m.0[5] = y;
        m.0[10] = z;
        m
    }

    pub fn translation(x: i32, y: i32, z: i32) -> Matrix {
        let mut m = Matrix::IDENTITY;
        m.0[12] = x;
        m.0[13] = y;
        m.0[14] = z;
        m
    }

    /// `(x, y, z, w) × self`, 20.12 in and out.
    #[inline]
    pub fn transform(&self, v: [i32; 4]) -> [i32; 4] {
        let m = &self.0;
        let mut out = [0i32; 4];
        for (col, o) in out.iter_mut().enumerate() {
            let acc = i64::from(v[0]) * i64::from(m[col])
                + i64::from(v[1]) * i64::from(m[4 + col])
                + i64::from(v[2]) * i64::from(m[8 + col])
                + i64::from(v[3]) * i64::from(m[12 + col]);
            *o = (acc >> 12) as i32;
        }
        out
    }

    /// `(x, y, z) × self` without the translation row: for normals and
    /// directions; the inputs keep whatever fixed point they came in.
    #[inline]
    pub fn transform_direction(&self, v: [i32; 3]) -> [i32; 3] {
        let m = &self.0;
        let mut out = [0i32; 3];
        for (col, o) in out.iter_mut().enumerate() {
            let acc = i64::from(v[0]) * i64::from(m[col])
                + i64::from(v[1]) * i64::from(m[4 + col])
                + i64::from(v[2]) * i64::from(m[8 + col]);
            *o = (acc >> 12) as i32;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_then_scale_applies_in_order() {
        // Current = T; then MTX_MULT with S makes Current = S × T: a vertex is
        // scaled first, then moved.
        let mut m = Matrix::translation(0x1000, 0, 0);
        m.mul_left(&Matrix::scale(0x2000, 0x2000, 0x2000));
        assert_eq!(m.transform([0x1000, 0, 0, 0x1000]), [0x3000, 0, 0, 0x1000]);
        // The other order moves first, then scales the whole thing.
        let mut m = Matrix::scale(0x2000, 0x2000, 0x2000);
        m.mul_left(&Matrix::translation(0x1000, 0, 0));
        assert_eq!(m.transform([0x1000, 0, 0, 0x1000]), [0x4000, 0, 0, 0x1000]);
    }
}
