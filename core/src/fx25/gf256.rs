//! GF(2^8) Galois Field arithmetic for Reed-Solomon codes.
//!
//! Uses primitive polynomial p(x) = x^8 + x^4 + x^3 + x^2 + 1 (0x11D),
//! which is the standard polynomial used by FX.25, CCSDS, and Dire Wolf.
//! Generator element alpha = 0x02.
//!
//! All arithmetic uses precomputed log/exp lookup tables (768 bytes in flash).

/// Primitive polynomial: x^8 + x^4 + x^3 + x^2 + 1
const PRIM_POLY: u16 = 0x11D;

/// Exponent (antilog) table: EXP[i] = alpha^i in GF(256).
/// Extended to 512 entries so `EXP[a + b]` works without modular reduction
/// for any `a, b` in 0..254.
static GF_EXP: [u8; 512] = {
    let mut table = [0u8; 512];
    let mut val: u16 = 1;
    let mut i = 0;
    while i < 512 {
        table[i] = val as u8;
        val <<= 1;
        if val & 0x100 != 0 {
            val ^= PRIM_POLY;
        }
        // After 255 iterations, val wraps to 1 (alpha^255 = 1 in GF(256))
        if i == 254 {
            val = 1;
        }
        i += 1;
    }
    table
};

/// Logarithm table: LOG[alpha^i] = i. LOG[0] = 0 (sentinel; never used in valid operations).
static GF_LOG: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut i = 0u16;
    while i < 255 {
        table[GF_EXP[i as usize] as usize] = i as u8;
        i += 1;
    }
    table
};

/// GF(256) addition: a + b = a XOR b.
#[inline(always)]
pub const fn gf_add(a: u8, b: u8) -> u8 {
    a ^ b
}

/// GF(256) subtraction: same as addition in GF(2^n).
#[inline(always)]
pub const fn gf_sub(a: u8, b: u8) -> u8 {
    a ^ b
}

/// GF(256) multiplication using log/exp tables.
#[inline]
pub fn gf_mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    GF_EXP[(GF_LOG[a as usize] as usize) + (GF_LOG[b as usize] as usize)]
}

/// GF(256) multiplicative inverse: a^(-1) such that a * a^(-1) = 1.
///
/// Panics if a == 0 (zero has no inverse).
#[inline]
pub fn gf_inv(a: u8) -> u8 {
    debug_assert!(a != 0, "GF(256): inverse of zero");
    GF_EXP[255 - GF_LOG[a as usize] as usize]
}

/// GF(256) division: a / b = a * b^(-1).
///
/// Panics if b == 0.
#[inline]
pub fn gf_div(a: u8, b: u8) -> u8 {
    if a == 0 {
        return 0;
    }
    debug_assert!(b != 0, "GF(256): division by zero");
    let log_a = GF_LOG[a as usize] as usize;
    let log_b = GF_LOG[b as usize] as usize;
    // Use extended table to avoid modular arithmetic
    GF_EXP[log_a + 255 - log_b]
}

/// GF(256) exponentiation: alpha^n.
#[inline]
pub fn gf_pow_alpha(n: u8) -> u8 {
    GF_EXP[n as usize]
}

/// Evaluate a polynomial at a given point in GF(256) using Horner's method.
///
/// `poly[0]` is the highest-degree coefficient, `poly[len-1]` is the constant term.
pub fn gf_poly_eval(poly: &[u8], x: u8) -> u8 {
    if poly.is_empty() {
        return 0;
    }
    let mut result = poly[0];
    for &coeff in &poly[1..] {
        result = gf_add(gf_mul(result, x), coeff);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_table_starts_correctly() {
        assert_eq!(GF_EXP[0], 1);   // alpha^0 = 1
        assert_eq!(GF_EXP[1], 2);   // alpha^1 = 2
        assert_eq!(GF_EXP[2], 4);   // alpha^2 = 4
        assert_eq!(GF_EXP[7], 128); // alpha^7 = 128
        // alpha^8 = 256 mod p(x) = 0x11D => 256 ^ 0x11D = 0x1D = 29
        assert_eq!(GF_EXP[8], 29);
    }

    #[test]
    fn exp_table_period_255() {
        // alpha^255 = 1, so EXP[255] should equal EXP[0] = 1
        assert_eq!(GF_EXP[255], 1);
        // Extended table should repeat
        for i in 0..255 {
            assert_eq!(GF_EXP[i], GF_EXP[i + 255], "EXP mismatch at {i}");
        }
    }

    #[test]
    fn log_exp_roundtrip() {
        for i in 0u16..255 {
            let val = GF_EXP[i as usize];
            assert_eq!(GF_LOG[val as usize], i as u8, "LOG/EXP roundtrip failed for i={i}");
        }
    }

    #[test]
    fn mul_identity() {
        for a in 0u16..=255 {
            assert_eq!(gf_mul(a as u8, 1), a as u8, "a * 1 != a for a={a}");
            assert_eq!(gf_mul(1, a as u8), a as u8, "1 * a != a for a={a}");
            assert_eq!(gf_mul(a as u8, 0), 0, "a * 0 != 0 for a={a}");
            assert_eq!(gf_mul(0, a as u8), 0, "0 * a != 0 for a={a}");
        }
    }

    #[test]
    fn mul_inverse() {
        for a in 1u16..=255 {
            let inv = gf_inv(a as u8);
            assert_eq!(gf_mul(a as u8, inv), 1, "a * inv(a) != 1 for a={a}");
        }
    }

    #[test]
    fn add_self_is_zero() {
        for a in 0u16..=255 {
            assert_eq!(gf_add(a as u8, a as u8), 0, "a + a != 0 for a={a}");
        }
    }

    #[test]
    fn div_by_self_is_one() {
        for a in 1u16..=255 {
            assert_eq!(gf_div(a as u8, a as u8), 1, "a / a != 1 for a={a}");
        }
    }

    #[test]
    fn mul_commutative() {
        // Spot check commutativity
        for a in [1u8, 2, 13, 42, 127, 200, 255] {
            for b in [1u8, 3, 17, 99, 128, 254] {
                assert_eq!(gf_mul(a, b), gf_mul(b, a), "mul not commutative: {a}*{b}");
            }
        }
    }

    #[test]
    fn poly_eval_constant() {
        assert_eq!(gf_poly_eval(&[42], 0), 42);
        assert_eq!(gf_poly_eval(&[42], 1), 42);
        assert_eq!(gf_poly_eval(&[42], 255), 42);
    }

    #[test]
    fn poly_eval_linear() {
        // p(x) = 3x + 5 (coeffs high-to-low: [3, 5])
        // p(0) = 5, p(1) = 3^1 = 3 XOR 5 = 6
        assert_eq!(gf_poly_eval(&[3, 5], 0), 5);
        assert_eq!(gf_poly_eval(&[3, 5], 1), gf_add(gf_mul(3, 1), 5));
    }
}
