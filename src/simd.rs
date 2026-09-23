//! SIMD-accelerated vector mathematics for high-performance retrieval.

#[inline]
pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }

    #[cfg(target_arch = "aarch64")]
    {
        // Safe to call NEON on all aarch64 (ARMv8-A baseline guarantee)
        unsafe { dot_product_neon(&a[..len], &b[..len]) }
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        dot_product_portable(&a[..len], &b[..len])
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn dot_product_neon(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::aarch64::*;

    let len = a.len();
    let mut sum_v0 = vdupq_n_f32(0.0);
    let mut sum_v1 = vdupq_n_f32(0.0);
    let mut sum_v2 = vdupq_n_f32(0.0);
    let mut sum_v3 = vdupq_n_f32(0.0);

    let mut i = 0;
    // Process 16 floats per iteration
    while i + 16 <= len {
        let va0 = vld1q_f32(a.as_ptr().add(i));
        let vb0 = vld1q_f32(b.as_ptr().add(i));
        sum_v0 = vfmaq_f32(sum_v0, va0, vb0);

        let va1 = vld1q_f32(a.as_ptr().add(i + 4));
        let vb1 = vld1q_f32(b.as_ptr().add(i + 4));
        sum_v1 = vfmaq_f32(sum_v1, va1, vb1);

        let va2 = vld1q_f32(a.as_ptr().add(i + 8));
        let vb2 = vld1q_f32(b.as_ptr().add(i + 8));
        sum_v2 = vfmaq_f32(sum_v2, va2, vb2);

        let va3 = vld1q_f32(a.as_ptr().add(i + 12));
        let vb3 = vld1q_f32(b.as_ptr().add(i + 12));
        sum_v3 = vfmaq_f32(sum_v3, va3, vb3);

        i += 16;
    }

    // Process remaining chunks of 4
    while i + 4 <= len {
        let va = vld1q_f32(a.as_ptr().add(i));
        let vb = vld1q_f32(b.as_ptr().add(i));
        sum_v0 = vfmaq_f32(sum_v0, va, vb);
        i += 4;
    }

    let combined = vaddq_f32(vaddq_f32(sum_v0, sum_v1), vaddq_f32(sum_v2, sum_v3));
    let mut sum = vaddvq_f32(combined);

    // Remainder
    while i < len {
        sum += *a.get_unchecked(i) * *b.get_unchecked(i);
        i += 1;
    }

    sum
}

#[inline]
pub fn dot_product_portable(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    let mut sum0 = 0.0f32;
    let mut sum1 = 0.0f32;
    let mut sum2 = 0.0f32;
    let mut sum3 = 0.0f32;

    let mut i = 0;
    while i + 4 <= len {
        sum0 += a[i] * b[i];
        sum1 += a[i + 1] * b[i + 1];
        sum2 += a[i + 2] * b[i + 2];
        sum3 += a[i + 3] * b[i + 3];
        i += 4;
    }

    let mut total = (sum0 + sum1) + (sum2 + sum3);
    while i < len {
        total += a[i] * b[i];
        i += 1;
    }
    total
}

/// Normalizes a vector in-place to unit L2 norm.
pub fn l2_normalize(vector: &mut [f32]) {
    let norm_sq = dot_product(vector, vector);
    if norm_sq > 0.0 {
        let inv_norm = 1.0 / norm_sq.sqrt();
        for val in vector.iter_mut() {
            *val *= inv_norm;
        }
    }
}

/// Computes cosine similarity between two slices.
pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }

    let dot = dot_product(left, right) as f64;
    let left_norm = dot_product(left, left) as f64;
    let right_norm = dot_product(right, right) as f64;

    if left_norm <= 0.0 || right_norm <= 0.0 {
        0.0
    } else {
        (dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(-1.0, 1.0)
    }
}

/// Converts a float vector to a little-endian byte slice.
pub fn vector_to_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Converts a byte slice to a float vector.
pub fn bytes_to_vector(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four bytes")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dot_product_accuracy() {
        let v1 = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let v2 = vec![2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        // 2 + 6 + 12 + 20 + 30 + 42 + 56 + 72 = 240
        let expected = 240.0f32;
        let actual = dot_product(&v1, &v2);
        assert!((actual - expected).abs() < 1e-4);
    }

    #[test]
    fn test_cosine_similarity() {
        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&v1, &v2) - 1.0).abs() < 1e-6);

        let v3 = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&v1, &v3).abs() < 1e-6);
    }
}
