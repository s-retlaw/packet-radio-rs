//! Reed-Solomon encoder and decoder over GF(256).
//!
//! Implements systematic RS(n, k) codes for FX.25:
//! - **Encoder**: Computes `n - k` parity bytes for a `k`-byte data block.
//! - **Decoder**: Syndrome computation, Berlekamp-Massey error locator,
//!   Chien search for error positions, Forney algorithm for error values.
//!
//! All operations use fixed-size stack buffers (max 64 check bytes = 32 errors).
//! No heap allocation required.
//!
//! Codeword layout: `[data_0, data_1, ..., data_{k-1}, par_0, par_1, ..., par_{nsym-1}]`
//! representing polynomial `data_0 * x^{n-1} + ... + par_{nsym-1} * x^0`.
//! (High-to-low memory order for the polynomial.)

use super::gf256::*;

/// Maximum number of check (parity) bytes supported.
pub const MAX_CHECK: usize = 64;

/// Maximum codeword length (RS over GF(256)).
pub const MAX_N: usize = 255;

/// RS encode/decode error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsError {
    /// Too many errors to correct.
    TooManyErrors,
    /// Chien search failed — error locator polynomial is inconsistent.
    ChienSearchFailed,
    /// Invalid parameters.
    InvalidParams,
}

/// Compute generator polynomial g(x) = prod(x - alpha^i) for i = 1..nsym.
///
/// Stored HIGH-TO-LOW: `gen[0]` = x^nsym coeff (= 1), `gen[nsym]` = constant.
/// Length = nsym + 1.
fn generator_poly(nsym: usize, gen: &mut [u8; MAX_CHECK + 1]) {
    // Start with g(x) = 1
    gen[0] = 1;
    for i in 1..=nsym {
        gen[i] = 0;
    }

    // Multiply by (x - alpha^i) for i = 1..nsym
    // In GF(2^n), (x - a) = (x + a), so:
    // g = g * (x + alpha^i)
    // new_g[j] = old_g[j-1] + alpha^i * old_g[j]  (shifting + scaling)
    for i in 1..=nsym {
        let ai = gf_pow_alpha(i as u8);
        // Process from the end to avoid overwriting
        for j in (1..=i).rev() {
            gen[j] = gf_add(gen[j - 1], gf_mul(ai, gen[j]));
        }
        gen[0] = gf_mul(ai, gen[0]);
        // Wait, gen[0] is the leading coefficient (should stay 1).
        // Let me reconsider: if gen is high-to-low, gen[0]=leading=1 initially.
        // Multiplying by (x + ai):
        //   If current poly is sum_{j=0}^{deg} gen[j] * x^{deg-j}
        //   New poly = x * current + ai * current
        //   New degree = deg + 1
        //   new[0] = old[0] (x^{deg+1} term from x*current)
        //   new[j] = old[j] + ai * old[j-1]  for j = 1..deg  (WAS WRONG ABOVE)
        //   Wait no... let me be very precise.
        // Actually the multiplication above had a bug. Let me redo it carefully.
    }
    // The above loop has a bug. Let me rewrite completely.

    // Clear and restart
    for i in 0..=MAX_CHECK {
        gen[i] = 0;
    }
    gen[0] = 1; // g(x) = 1, stored as gen[0] = coeff of x^0

    // Build g(x) = (x + alpha^1)(x + alpha^2)...(x + alpha^nsym)
    // After each multiplication, the degree increases by 1.
    // We maintain gen as high-to-low during construction.
    let mut deg = 0usize;

    for i in 1..=nsym {
        let ai = gf_pow_alpha(i as u8);
        deg += 1;
        // Multiply current g(x) of degree (deg-1) by (x + ai) to get degree deg.
        // High-to-low: gen[0..deg-1] are current coefficients.
        // new[0] = old[0]  (leading coeff of x * g)
        // new[j] = old[j] + ai * old[j-1] for j = 1..deg-1
        // new[deg] = ai * old[deg-1]
        // Process from back to front:
        gen[deg] = gf_mul(ai, gen[deg - 1]);
        for j in (1..deg).rev() {
            gen[j] = gf_add(gen[j], gf_mul(ai, gen[j - 1]));
        }
        // gen[0] stays the same (leading coefficient, always 1)
    }
}

/// Encode a data block using systematic RS(n, k).
///
/// Uses polynomial long division: computes remainder of `data(x) * x^nsym / g(x)`.
///
/// - `data`: k-byte message
/// - `nsym`: number of check bytes (n - k)
/// - `parity_out`: output buffer for `nsym` parity bytes
///
/// Codeword = `[data | parity_out]`.
pub fn rs_encode(data: &[u8], nsym: usize, parity_out: &mut [u8]) -> Result<(), RsError> {
    if nsym > MAX_CHECK || nsym == 0 || parity_out.len() < nsym {
        return Err(RsError::InvalidParams);
    }

    let mut gen = [0u8; MAX_CHECK + 1];
    generator_poly(nsym, &mut gen);
    // gen is high-to-low, length nsym+1, gen[0] = 1

    // Long division: divide [data | 0...0] by gen.
    // Work on a copy: dividend = [data_0, ..., data_{k-1}, 0, ..., 0] (k + nsym bytes)
    // After division, the last nsym bytes are the remainder = parity.
    let k = data.len();
    let n = k + nsym;
    let mut dividend = [0u8; MAX_N];
    dividend[..k].copy_from_slice(data);
    // dividend[k..n] already zero

    for i in 0..k {
        let coeff = dividend[i];
        if coeff != 0 {
            // Subtract coeff * gen from dividend[i..i+nsym+1]
            // gen[0] = 1, so dividend[i] -= coeff * 1 = 0 (intended)
            for j in 1..=nsym {
                dividend[i + j] = gf_add(dividend[i + j], gf_mul(coeff, gen[j]));
            }
        }
    }

    // The remainder is in dividend[k..n]
    parity_out[..nsym].copy_from_slice(&dividend[k..n]);
    Ok(())
}

/// Compute syndromes S[i] = r(alpha^{i+1}) for i = 0..nsym-1.
///
/// Codeword is in high-to-low memory order: `codeword[0]` = coeff of x^{n-1}.
/// Returns true if all syndromes are zero (no errors).
fn compute_syndromes(codeword: &[u8], nsym: usize, syndromes: &mut [u8]) -> bool {
    let mut all_zero = true;
    for s in 0..nsym {
        let x = gf_pow_alpha((s + 1) as u8);
        // Horner evaluation: high-to-low order
        let mut val = 0u8;
        for &c in codeword {
            val = gf_add(gf_mul(val, x), c);
        }
        syndromes[s] = val;
        if val != 0 {
            all_zero = false;
        }
    }
    all_zero
}

/// Berlekamp-Massey: find error locator polynomial Lambda(x).
///
/// Lambda stored low-to-high: `lambda[0]` = 1 (constant), `lambda[i]` = x^i coeff.
/// Returns number of errors detected.
fn berlekamp_massey(syndromes: &[u8], nsym: usize, lambda: &mut [u8]) -> usize {
    let mut c = [0u8; MAX_CHECK + 1];
    let mut b = [0u8; MAX_CHECK + 1];
    c[0] = 1;
    b[0] = 1;

    let mut l: usize = 0;
    let mut m: usize = 1;
    let mut b_val: u8 = 1;

    for n in 0..nsym {
        let mut delta = syndromes[n];
        for i in 1..=l {
            delta = gf_add(delta, gf_mul(c[i], syndromes[n - i]));
        }

        if delta == 0 {
            m += 1;
        } else if 2 * l <= n {
            let mut t = [0u8; MAX_CHECK + 1];
            t[..=nsym].copy_from_slice(&c[..=nsym]);
            let factor = gf_div(delta, b_val);
            for i in m..=nsym {
                c[i] = gf_add(c[i], gf_mul(factor, b[i - m]));
            }
            l = n + 1 - l;
            b[..=nsym].copy_from_slice(&t[..=nsym]);
            b_val = delta;
            m = 1;
        } else {
            let factor = gf_div(delta, b_val);
            for i in m..=nsym {
                c[i] = gf_add(c[i], gf_mul(factor, b[i - m]));
            }
            m += 1;
        }
    }

    lambda[..=l].copy_from_slice(&c[..=l]);
    l
}

/// Evaluate a polynomial stored low-to-high at point `x`.
fn poly_eval_lh(poly: &[u8], x: u8) -> u8 {
    // p(x) = poly[0] + poly[1]*x + poly[2]*x^2 + ...
    // Horner from high end: result = (...((poly[n]*x + poly[n-1])*x + ...)...)
    let mut result = 0u8;
    for &coeff in poly.iter().rev() {
        result = gf_add(gf_mul(result, x), coeff);
    }
    result
}

/// Chien search: find error positions by evaluating Lambda(alpha^{-j}).
///
/// BM produces Lambda(x) = prod(1 - X_k * x) with roots at X_k^{-1}.
/// If Lambda(alpha^{-j}) = 0, then X_k = alpha^j = alpha^{n-1-pos},
/// so the error is at codeword[n-1-j].
fn chien_search(
    lambda: &[u8],
    num_errors: usize,
    n: usize,
    positions: &mut [u8],
) -> Result<usize, RsError> {
    let mut found = 0;
    for j in 0..n {
        // alpha^{-j}: for j=0 → alpha^0=1, j>0 → alpha^{255-j}
        let exp = if j == 0 { 0 } else { 255 - j };
        let x = gf_pow_alpha(exp as u8);
        if poly_eval_lh(&lambda[..=num_errors], x) == 0 {
            let pos = n - 1 - j;
            positions[found] = pos as u8;
            found += 1;
            if found == num_errors {
                break;
            }
        }
    }
    if found != num_errors {
        return Err(RsError::ChienSearchFailed);
    }
    Ok(found)
}

/// Forney algorithm: compute error magnitudes.
///
/// Given Lambda (low-to-high), syndromes, and error positions (byte indices),
/// compute the error value at each position.
fn forney(
    syndromes: &[u8],
    lambda: &[u8],
    num_errors: usize,
    positions: &[u8],
    n: usize,
    nsym: usize,
    magnitudes: &mut [u8],
) {
    // Error evaluator Omega(x) = S(x) * Lambda(x) mod x^nsym
    // S(x) = S[0] + S[1]*x + S[2]*x^2 + ... (low-to-high)
    let mut omega = [0u8; MAX_CHECK];
    for i in 0..nsym {
        let mut val = 0u8;
        for j in 0..=i.min(num_errors) {
            val = gf_add(val, gf_mul(syndromes[i - j], lambda[j]));
        }
        omega[i] = val;
    }

    for (idx, &pos) in positions[..num_errors].iter().enumerate() {
        // Error locator X_k = alpha^{n-1-pos}.
        // X_k^{-1} = alpha^{-(n-1-pos)}.
        let power = (n - 1 - pos as usize) % 255;
        let x_k_inv_exp = if power == 0 { 0 } else { 255 - power };
        let x_k_inv = gf_pow_alpha(x_k_inv_exp as u8);

        // Omega(X_k^{-1})
        let omega_val = poly_eval_lh(&omega[..nsym], x_k_inv);

        // Lambda'(X_k^{-1}): formal derivative, only odd-indexed terms in char 2
        // Lambda'(x) = lambda[1] + lambda[3]*x^2 + lambda[5]*x^4 + ...
        let mut lprime_val = 0u8;
        let mut i = 1;
        while i <= num_errors {
            let mut x_pow = 1u8;
            for _ in 0..i - 1 {
                x_pow = gf_mul(x_pow, x_k_inv);
            }
            lprime_val = gf_add(lprime_val, gf_mul(lambda[i], x_pow));
            i += 2;
        }

        if lprime_val == 0 {
            magnitudes[idx] = 0;
            continue;
        }

        // Forney formula with FCR=1: e_k = Omega(X_k^{-1}) / Lambda'(X_k^{-1})
        magnitudes[idx] = gf_div(omega_val, lprime_val);
    }
}

/// Decode a Reed-Solomon codeword in place.
///
/// - `codeword`: the n-byte block `[data | parity]` (corrected in place)
/// - `n`: total codeword length
/// - `nsym`: number of check bytes (n - k)
///
/// Returns number of corrected byte errors, or `Err` if uncorrectable.
pub fn rs_decode(codeword: &mut [u8], n: usize, nsym: usize) -> Result<usize, RsError> {
    if nsym > MAX_CHECK || nsym == 0 || n > MAX_N {
        return Err(RsError::InvalidParams);
    }

    // 1. Syndromes
    let mut syndromes = [0u8; MAX_CHECK];
    if compute_syndromes(&codeword[..n], nsym, &mut syndromes) {
        return Ok(0);
    }

    // 2. Berlekamp-Massey
    let mut lambda = [0u8; MAX_CHECK + 1];
    let num_errors = berlekamp_massey(&syndromes, nsym, &mut lambda);
    if num_errors == 0 || num_errors > nsym / 2 {
        return Err(RsError::TooManyErrors);
    }

    // 3. Chien search
    let mut positions = [0u8; MAX_CHECK];
    chien_search(&lambda, num_errors, n, &mut positions)?;

    // 4. Forney
    let mut magnitudes = [0u8; MAX_CHECK];
    forney(&syndromes[..nsym], &lambda, num_errors, &positions, n, nsym, &mut magnitudes);

    // 5. Correct
    for i in 0..num_errors {
        let pos = positions[i] as usize;
        if pos >= n {
            return Err(RsError::TooManyErrors);
        }
        codeword[pos] = gf_add(codeword[pos], magnitudes[i]);
    }

    // 6. Verify
    let mut verify = [0u8; MAX_CHECK];
    if !compute_syndromes(&codeword[..n], nsym, &mut verify) {
        return Err(RsError::TooManyErrors);
    }

    Ok(num_errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_test_block(data: &[u8], nsym: usize) -> ([u8; MAX_N], usize) {
        let n = data.len() + nsym;
        let mut codeword = [0u8; MAX_N];
        codeword[..data.len()].copy_from_slice(data);
        let mut parity = [0u8; MAX_CHECK];
        rs_encode(data, nsym, &mut parity).unwrap();
        codeword[data.len()..n].copy_from_slice(&parity[..nsym]);
        (codeword, n)
    }

    #[test]
    fn encode_produces_valid_codeword() {
        let data = b"Hello, FX.25!";
        let nsym = 16;
        let (codeword, n) = encode_test_block(data, nsym);
        let mut syndromes = [0u8; MAX_CHECK];
        assert!(compute_syndromes(&codeword[..n], nsym, &mut syndromes),
            "codeword has non-zero syndromes");
    }

    #[test]
    fn encode_decode_no_errors() {
        let data = b"Hello, FX.25 Reed-Solomon!";
        let nsym = 16;
        let (mut codeword, n) = encode_test_block(data, nsym);
        let corrected = rs_decode(&mut codeword, n, nsym).unwrap();
        assert_eq!(corrected, 0);
        assert_eq!(&codeword[..data.len()], data);
    }

    #[test]
    fn encode_decode_single_error() {
        let data = b"Test data for RS correction";
        let nsym = 16;
        let (mut codeword, n) = encode_test_block(data, nsym);
        codeword[5] ^= 0xAB;
        let corrected = rs_decode(&mut codeword, n, nsym).unwrap();
        assert_eq!(corrected, 1);
        assert_eq!(&codeword[..data.len()], data);
    }

    #[test]
    fn encode_decode_max_errors_16check() {
        let data = b"Correct up to 8 byte errors with 16 check bytes!!";
        let nsym = 16;
        let max_t = nsym / 2;
        let (mut codeword, n) = encode_test_block(data, nsym);
        for i in 0..max_t {
            codeword[i * 3] ^= (0x11 + i) as u8;
        }
        let corrected = rs_decode(&mut codeword, n, nsym).unwrap();
        assert_eq!(corrected, max_t);
        assert_eq!(&codeword[..data.len()], data);
    }

    #[test]
    fn encode_decode_max_errors_32check() {
        let data = b"RS(n,k) with 32 check bytes corrects 16 byte errors";
        let nsym = 32;
        let max_t = nsym / 2;
        let (mut codeword, n) = encode_test_block(data, nsym);
        for i in 0..max_t {
            codeword[i * 2 + 1] ^= (0x55 + i) as u8;
        }
        let corrected = rs_decode(&mut codeword, n, nsym).unwrap();
        assert_eq!(corrected, max_t);
        assert_eq!(&codeword[..data.len()], data);
    }

    #[test]
    fn decode_too_many_errors() {
        let data = b"Too many errors to fix";
        let nsym = 16;
        let (mut codeword, n) = encode_test_block(data, nsym);
        for i in 0..9 {
            codeword[i] ^= 0xFF;
        }
        assert!(rs_decode(&mut codeword, n, nsym).is_err());
    }

    #[test]
    fn encode_decode_parity_corruption() {
        let data = b"Errors in parity bytes";
        let nsym = 16;
        let (mut codeword, n) = encode_test_block(data, nsym);
        for i in 0..8 {
            codeword[data.len() + i] ^= 0xCC;
        }
        let corrected = rs_decode(&mut codeword, n, nsym).unwrap();
        assert_eq!(corrected, 8);
        assert_eq!(&codeword[..data.len()], data);
    }

    #[test]
    fn encode_decode_fx25_code_sizes() {
        let test_data = [0x42u8; 200];
        for &(rs_k, nsym) in &[
            (239u16, 16usize), (128, 16), (64, 16), (32, 16),
            (223, 32), (128, 32), (64, 32), (32, 32),
            (191, 64), (128, 64), (64, 64),
        ] {
            let k = rs_k as usize;
            let data = &test_data[..k.min(200)];
            let (mut codeword, n) = encode_test_block(data, nsym);
            let errors = nsym / 4; // half of max correctable
            for i in 0..errors {
                codeword[i * 5 % n] ^= 0x37;
            }
            let corrected = rs_decode(&mut codeword, n, nsym).unwrap();
            assert_eq!(corrected, errors, "RS({n},{k}) with {errors} errors");
            assert_eq!(&codeword[..data.len()], data, "RS({n},{k}) data mismatch");
        }
    }

    #[test]
    fn roundtrip_all_single_byte_errors() {
        let data = b"short";
        let nsym = 4;
        let (original, n) = encode_test_block(data, nsym);
        for pos in 0..n {
            for err_val in 1u16..=255 {
                let mut codeword = original;
                codeword[pos] ^= err_val as u8;
                let corrected = rs_decode(&mut codeword, n, nsym)
                    .unwrap_or_else(|e| panic!("pos={pos} err=0x{err_val:02X}: {e:?}"));
                assert_eq!(corrected, 1);
                assert_eq!(&codeword[..data.len()], data,
                    "pos={pos}, err=0x{err_val:02X}");
            }
        }
    }
}
