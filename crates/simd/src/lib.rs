//! Hardware estimates that the Cranelift backend cannot lower itself.

#![forbid(unsafe_code)]

/// A raw reciprocal-square-root estimate, before Newton refinement.
///
/// On x86-64 this is the CPU's `rsqrtps` estimate, not a correctly rounded
/// reciprocal square root. Different CPU implementations can disagree.
/// Other architectures retain a correctly rounded seed; it does not emulate
/// an x86 client's estimate table.
///
/// Keep this non-generic and out of line: the dev profile compiles this small
/// crate with LLVM so Cranelift callers never have to lower `rsqrtps`.
#[inline(never)]
pub fn reciprocal_sqrt_estimate(squared: f32) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        use safe_arch::{get_f32_from_m128_s, m128, reciprocal_sqrt_m128};
        get_f32_from_m128_s(reciprocal_sqrt_m128(m128::from_array([squared; 4])))
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        (1.0 / f64::from(squared).sqrt()) as f32
    }
}
