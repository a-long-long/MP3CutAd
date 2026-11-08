//! LSH (Locality Sensitive Hashing) 模块
//!
//! 使用随机超平面进行局部敏感哈希，用于快速查找相似的频谱片段

use ndarray::Array2;
use rand::rngs::StdRng;
use rand::Rng;
use rand::SeedableRng;
use rayon::prelude::*;
use std::sync::OnceLock;

/// 随机投影向量全局存储
static VECTORS: OnceLock<Vec<Array2<f64>>> = OnceLock::new();

/// 哈希位数（增加到 16 位以提高区分度）
const NUM_HASH_BITS: usize = 16;
/// 多探针 LSH 的探针半径（检查相差 ≤ 此位数的哈希）
pub const PROBE_RADIUS: usize = 2;

/// 初始化 LSH 随机向量
///
/// # 参数
/// - `time_size`: 时间窗口大小
/// - `freq_size`: 频率 bin 数量
pub fn init(time_size: usize, freq_size: usize) {
    VECTORS.get_or_init(|| {
        let mut rng = StdRng::seed_from_u64(1);
        let mut vectors = Vec::with_capacity(NUM_HASH_BITS);

        for _ in 0..NUM_HASH_BITS {
            let mut vec = Array2::zeros((time_size, freq_size));
            for val in vec.iter_mut() {
                *val = rng.gen::<f64>() - 0.5;
            }
            vectors.push(vec);
        }

        vectors
    });
}

/// 计算两个频谱片段的余弦相似度（自动选择最优实现）
///
/// # 参数
/// - `a`: 频谱图 A
/// - `b`: 频谱图 B
/// - `offset_a`: A 中的起始帧
/// - `offset_b`: B 中的起始帧
/// - `size`: 比较的帧数
///
/// # 返回
/// 余弦相似度，范围 [-1, 1]，值越大越相似
#[inline]
pub fn cosine_sim(
    a: &Array2<f64>,
    b: &Array2<f64>,
    offset_a: usize,
    offset_b: usize,
    size: usize,
) -> f64 {
    let width = a.ncols();
    debug_assert_eq!(width, b.ncols());
    debug_assert!(offset_a + size <= a.nrows());
    debug_assert!(offset_b + size <= b.nrows());

    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: 运行时已确认 CPU 支持 AVX2
            return unsafe { cosine_sim_avx2(a, b, offset_a, offset_b, size, width) };
        }
    }

    cosine_sim_scalar(a, b, offset_a, offset_b, size, width)
}

/// 标量版本：使用平坦内存访问，便于编译器自动向量化
#[inline]
fn cosine_sim_scalar(
    a: &Array2<f64>,
    b: &Array2<f64>,
    offset_a: usize,
    offset_b: usize,
    size: usize,
    width: usize,
) -> f64 {
    let a_slice = a.as_slice().expect("FFT 数组必须是连续存储");
    let b_slice = b.as_slice().expect("FFT 数组必须是连续存储");
    let total = size * width;
    let a_data = &a_slice[offset_a * width..offset_a * width + total];
    let b_data = &b_slice[offset_b * width..offset_b * width + total];

    let mut dot_product = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;

    for i in 0..total {
        let va = a_data[i];
        let vb = b_data[i];
        dot_product += va * vb;
        norm_a += va * va;
        norm_b += vb * vb;
    }

    let denom = (norm_a * norm_b).sqrt();
    if denom < 1e-8 {
        0.0
    } else {
        dot_product / denom
    }
}

/// AVX2 优化版本：每次处理 4 个 f64（256 位），使用平坦内存访问
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn cosine_sim_avx2(
    a: &Array2<f64>,
    b: &Array2<f64>,
    offset_a: usize,
    offset_b: usize,
    size: usize,
    width: usize,
) -> f64 {
    use std::arch::x86_64::*;

    let total = size * width;
    let a_ptr = a.as_ptr().add(offset_a * width);
    let b_ptr = b.as_ptr().add(offset_b * width);

    let mut f_acc = _mm256_setzero_pd();
    let mut na_acc = _mm256_setzero_pd();
    let mut nb_acc = _mm256_setzero_pd();

    // 主循环：每次处理 4 个 f64
    let mut i = 0;
    while i + 4 <= total {
        let va = _mm256_loadu_pd(a_ptr.add(i));
        let vb = _mm256_loadu_pd(b_ptr.add(i));

        f_acc = _mm256_add_pd(f_acc, _mm256_mul_pd(va, vb));
        na_acc = _mm256_add_pd(na_acc, _mm256_mul_pd(va, va));
        nb_acc = _mm256_add_pd(nb_acc, _mm256_mul_pd(vb, vb));

        i += 4;
    }

    // 处理剩余不足 4 个的元素
    let mut f_rem = 0.0f64;
    let mut na_rem = 0.0f64;
    let mut nb_rem = 0.0f64;
    while i < total {
        let va = *a_ptr.add(i);
        let vb = *b_ptr.add(i);
        f_rem += va * vb;
        na_rem += va * va;
        nb_rem += vb * vb;
        i += 1;
    }

    // 水平归约：将 4 个 lane 的值求和
    let mut f_buf = [0.0f64; 4];
    let mut na_buf = [0.0f64; 4];
    let mut nb_buf = [0.0f64; 4];
    _mm256_storeu_pd(f_buf.as_mut_ptr(), f_acc);
    _mm256_storeu_pd(na_buf.as_mut_ptr(), na_acc);
    _mm256_storeu_pd(nb_buf.as_mut_ptr(), nb_acc);

    let f = f_buf.iter().sum::<f64>() + f_rem;
    let na = na_buf.iter().sum::<f64>() + na_rem;
    let nb = nb_buf.iter().sum::<f64>() + nb_rem;

    let denom = (na * nb).sqrt();
    if denom < 1e-8 {
        0.0
    } else {
        f / denom
    }
}

/// 计算频谱片段的 LSH 哈希值
///
/// # 参数
/// - `a`: 频谱图
/// - `offset`: 起始帧位置
///
/// # 返回
/// 哈希值（16 位整数）
pub fn hash(a: &Array2<f64>, offset: usize) -> i32 {
    let vectors = VECTORS.get().expect("LSH 未初始化");
    let mut ret: i32 = 0;

    for vec in vectors {
        let sim = cosine_sim(a, vec, offset, 0, vec.nrows());
        ret <<= 1;
        if sim <= 0.0 {
            ret |= 1;
        }
    }

    ret
}

/// 批量计算哈希值（使用 rayon 并行加速）
pub fn hash_all(a: &Array2<f64>, window_size: usize) -> Vec<i32> {
    let len = a.nrows();
    if len < window_size {
        return Vec::new();
    }

    (0..len - window_size)
        .into_par_iter()
        .map(|i| hash(a, i))
        .collect()
}

/// 多探针 LSH：生成给定哈希的所有邻近探针
///
/// 返回与 `base_hash` 汉明距离 ≤ `PROBE_RADIUS` 的所有哈希值。
/// 用于提高召回率：即使哈希因噪声翻转了几位，仍能找到匹配。
pub fn multi_probe_hashes(base_hash: i32) -> Vec<i32> {
    let mut probes = vec![base_hash];

    if PROBE_RADIUS >= 1 {
        // 距离 1：翻转单个 bit
        for bit in 0..NUM_HASH_BITS {
            probes.push(base_hash ^ (1 << bit));
        }
    }

    if PROBE_RADIUS >= 2 {
        // 距离 2：翻转两个 bit
        for b1 in 0..NUM_HASH_BITS {
            for b2 in (b1 + 1)..NUM_HASH_BITS {
                probes.push(base_hash ^ (1 << b1) ^ (1 << b2));
            }
        }
    }

    probes
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array2;

    #[test]
    fn test_cosine_sim_identical() {
        init(10, 32);

        let a: Array2<f64> = Array2::from_shape_fn((20, 32), |(i, j)| ((i * 32 + j) as f64).sin());

        // 相同片段的相似度应为 1
        let sim = cosine_sim(&a, &a, 0, 0, 10);
        assert!((sim - 1.0).abs() < 1e-6, "sim = {}", sim);
    }

    #[test]
    fn test_cosine_sim_orthogonal() {
        init(10, 32);

        let a: Array2<f64> = Array2::from_shape_fn((20, 32), |(i, _)| (i as f64).sin());

        let b: Array2<f64> = Array2::from_shape_fn((20, 32), |(i, _)| (i as f64).cos());

        // 正交片段的相似度应接近 0
        let sim = cosine_sim(&a, &b, 0, 0, 10);
        assert!(sim.abs() < 0.5, "sim = {}", sim);
    }

    #[test]
    fn test_hash_consistency() {
        init(10, 32);

        let a: Array2<f64> = Array2::from_shape_fn((20, 32), |(i, j)| ((i * 32 + j) as f64).sin());

        // 相同输入应产生相同哈希
        let h1 = hash(&a, 0);
        let h2 = hash(&a, 0);
        assert_eq!(h1, h2);
    }
}
