//! GPU 加速的批量余弦相似度（可选 `gpu` feature）
//!
//! 把「两个频谱图 + 一批 (帧A, 帧B) 偏移」的余弦相似度放到显卡上并行计算，
//! 用于检测阶段的候选帧比对。未编译 `gpu` feature、找不到可用显卡或初始化失败时，
//! 所有接口返回 `None`，调用方自动回退 CPU 实现。
//!
//! 可通过环境变量 `MP3CUTAD_NO_GPU=1` 强制关闭。

#[cfg(not(feature = "gpu"))]
mod imp {
    use ndarray::Array2;

    /// GPU 后端名称（未启用时返回 None）
    pub fn backend_name() -> Option<String> {
        None
    }

    /// 批量余弦相似度（未启用 GPU 时返回 None，由调用方走 CPU）
    pub fn cosine_pairs(
        _a: &Array2<f64>,
        _b: &Array2<f64>,
        _pairs: &[(usize, usize)],
        _size: usize,
    ) -> Option<Vec<f64>> {
        None
    }
}

#[cfg(feature = "gpu")]
mod imp {
    use std::sync::OnceLock;

    use ndarray::Array2;

    /// 单个批次的最大配对数，避免一次提交过大
    const MAX_PAIRS_PER_BATCH: usize = 1 << 18;
    /// 配对数量低于此值时直接走 CPU，避免调度开销
    const MIN_PAIRS_FOR_GPU: usize = 512;

    #[repr(C)]
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    struct Params {
        width: u32,
        size: u32,
        count: u32,
        _pad: u32,
    }

    struct Backend {
        device: wgpu::Device,
        queue: wgpu::Queue,
        pipeline: wgpu::ComputePipeline,
        bind_group_layout: wgpu::BindGroupLayout,
        name: String,
    }

    static BACKEND: OnceLock<Option<Backend>> = OnceLock::new();

    fn backend() -> Option<&'static Backend> {
        BACKEND
            .get_or_init(|| {
                if std::env::var("MP3CUTAD_NO_GPU").map(|v| v == "1").unwrap_or(false) {
                    return None;
                }
                init_backend()
            })
            .as_ref()
    }

    /// GPU 后端名称（如 `NVIDIA GeForce RTX 3060`）
    pub fn backend_name() -> Option<String> {
        backend().map(|b| b.name.clone())
    }

    fn init_backend() -> Option<Backend> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))?;

        let info = adapter.get_info();
        // 默认的 downlevel 限制只允许 4 个 storage buffer，本管线需要 6 个
        let mut limits = wgpu::Limits::downlevel_defaults();
        limits.max_storage_buffers_per_shader_stage = 8;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("mp3cutad-gpu"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                ..Default::default()
            },
            None,
        ))
        .ok()?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cosine_pairs"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cosine_pairs_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage_entry(1, false),
                storage_entry(2, false),
                storage_entry(3, false),
                storage_entry(4, false),
                storage_entry(5, false),
                storage_entry(6, true),
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cosine_pairs_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cosine_pairs_pipeline"),
            layout: Some(&layout),
            module: &shader,
            entry_point: "main",
            compilation_options: Default::default(),
        });

        Some(Backend {
            device,
            queue,
            pipeline,
            bind_group_layout,
            name: info.name,
        })
    }

    fn storage_entry(binding: u32, read_write: bool) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: !read_write },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }
    }

    /// 每帧窗口的能量平方和（GPU 侧只需开方求分母）
    fn window_norms(m: &Array2<f64>, size: usize) -> Vec<f32> {
        let rows = m.nrows();
        let mut prefix = vec![0.0f64; rows + 1];
        for r in 0..rows {
            let s: f64 = m.row(r).iter().map(|v| v * v).sum();
            prefix[r + 1] = prefix[r] + s;
        }
        (0..=rows.saturating_sub(size))
            .map(|k| (prefix[k + size] - prefix[k]) as f32)
            .collect()
    }

    fn to_f32(m: &Array2<f64>) -> Option<Vec<f32>> {
        m.as_slice().map(|s| s.iter().map(|&v| v as f32).collect())
    }

    /// 批量计算余弦相似度；不可用时返回 None
    pub fn cosine_pairs(
        a: &Array2<f64>,
        b: &Array2<f64>,
        pairs: &[(usize, usize)],
        size: usize,
    ) -> Option<Vec<f64>> {
        if pairs.len() < MIN_PAIRS_FOR_GPU || a.ncols() != b.ncols() {
            return None;
        }
        // window_norms 依赖 size <= 行数；候选下标也必须能容纳完整窗口
        if size == 0 || size > a.nrows() || size > b.nrows() {
            return None;
        }
        if pairs.iter().any(|&(i, j)| {
            i.checked_add(size).is_none_or(|e| e > a.nrows())
                || j.checked_add(size).is_none_or(|e| e > b.nrows())
        }) {
            return None;
        }
        let width = a.ncols();
        let a_data = to_f32(a)?;
        let b_data = to_f32(b)?;
        let norms_a = window_norms(a, size);
        let norms_b = window_norms(b, size);

        let backend = backend()?;
        let mut out = Vec::with_capacity(pairs.len());
        for chunk in pairs.chunks(MAX_PAIRS_PER_BATCH) {
            let scores = run_batch(backend, &a_data, &b_data, &norms_a, &norms_b, chunk, size, width)?;
            out.extend(scores);
        }
        Some(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_batch(
        backend: &Backend,
        a: &[f32],
        b: &[f32],
        norms_a: &[f32],
        norms_b: &[f32],
        pairs: &[(usize, usize)],
        size: usize,
        width: usize,
    ) -> Option<Vec<f64>> {
        let device = &backend.device;
        let queue = &backend.queue;

        let pair_data: Vec<[u32; 2]> = pairs.iter().map(|&(i, p)| [i as u32, p as u32]).collect();
        let params = Params {
            width: width as u32,
            size: size as u32,
            count: pairs.len() as u32,
            _pad: 0,
        };

        let a_buf = create_storage(device, "fft_a", bytemuck::cast_slice(a));
        let b_buf = create_storage(device, "fft_b", bytemuck::cast_slice(b));
        let pair_buf = create_storage(device, "pairs", bytemuck::cast_slice(&pair_data));
        let na_buf = create_storage(device, "norms_a", bytemuck::cast_slice(norms_a));
        let nb_buf = create_storage(device, "norms_b", bytemuck::cast_slice(norms_b));
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&params_buf, 0, bytemuck::bytes_of(&params));

        let out_size = (pairs.len() * std::mem::size_of::<f32>()) as u64;
        let out_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scores"),
            size: out_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scores_staging"),
            size: out_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cosine_pairs_bg"),
            layout: &backend.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: a_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: b_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: pair_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: na_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: nb_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: out_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("cosine_pairs_encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cosine_pairs_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&backend.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let groups = (pairs.len() as u32 + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
            pass.dispatch_workgroups(groups, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&out_buf, 0, &staging, 0, out_size);
        queue.submit(Some(encoder.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().ok()?.ok()?;

        let data = slice.get_mapped_range();
        let scores: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();

        Some(scores.into_iter().map(|v| v as f64).collect())
    }

    fn create_storage(device: &wgpu::Device, label: &str, bytes: &[u8]) -> wgpu::Buffer {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.len().max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        if !bytes.is_empty() {
            buf.slice(..).get_mapped_range_mut()[..bytes.len()].copy_from_slice(bytes);
        }
        buf.unmap();
        buf
    }

    const WORKGROUP_SIZE: u32 = 64;

    const SHADER: &str = r#"
struct Params {
    width: u32,
    size: u32,
    count: u32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> a: array<f32>;
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read> pairs: array<vec2<u32>>;
@group(0) @binding(4) var<storage, read> norms_a: array<f32>;
@group(0) @binding(5) var<storage, read> norms_b: array<f32>;
@group(0) @binding(6) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= params.count) {
        return;
    }
    let pair = pairs[idx];
    let base_a = pair.x * params.width;
    let base_b = pair.y * params.width;
    let total = params.size * params.width;

    var dot = 0.0;
    for (var k = 0u; k < total; k = k + 1u) {
        dot = dot + a[base_a + k] * b[base_b + k];
    }

    let denom = sqrt(norms_a[pair.x] * norms_b[pair.y]);
    if (denom < 1e-8) {
        out[idx] = 0.0;
    } else {
        out[idx] = dot / denom;
    }
}
"#;
}

pub use imp::{backend_name, cosine_pairs};

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;
    use crate::core::matching::lsh;
    use ndarray::Array2;

    /// GPU 结果需与 CPU 余弦相似度一致；无可用显卡时跳过
    #[test]
    fn gpu_matches_cpu_scores() {
        if backend_name().is_none() {
            eprintln!("未检测到可用 GPU，跳过");
            return;
        }

        let size = 8;
        let width = 32;
        let a: Array2<f64> =
            Array2::from_shape_fn((64, width), |(i, j)| ((i * 7 + j * 13) as f64 * 0.017).sin());
        let b: Array2<f64> =
            Array2::from_shape_fn((64, width), |(i, j)| ((i * 5 + j * 11) as f64 * 0.023).cos());

        // 构造足够多的配对以触发 GPU 路径
        let mut pairs = Vec::with_capacity(4096);
        for k in 0..4096usize {
            pairs.push((k % 40, (k * 3 + 7) % 40));
        }

        let scores = cosine_pairs(&a, &b, &pairs, size).expect("GPU 应返回结果");
        assert_eq!(scores.len(), pairs.len());

        for (idx, &(i, p)) in pairs.iter().enumerate().take(256) {
            let cpu = lsh::cosine_sim(&a, &b, i, p, size);
            assert!(
                (scores[idx] - cpu).abs() < 1e-4,
                "pair {}: gpu={} cpu={}",
                idx,
                scores[idx],
                cpu
            );
        }
    }

    #[test]
    fn small_batches_fall_back_to_cpu() {
        let a: Array2<f64> = Array2::zeros((8, 8));
        let b: Array2<f64> = Array2::zeros((8, 8));
        assert!(cosine_pairs(&a, &b, &[(0, 0)], 4).is_none());
    }
}
