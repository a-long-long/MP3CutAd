
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use super::forest::Forest;
use super::segfeat::{SEG_FEATURE_DIM, SEG_SR};

pub const MODEL_MAGIC: &[u8; 4] = b"MCRF";
pub const MODEL_VERSION: u32 = 1;
pub const MAX_TREES: usize = 4096;
pub const MAX_NODES: usize = 64_000_000;
pub const MAX_LABELS: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelFile {
    pub version: u32,
    pub feature_dim: usize,
    pub labels: Vec<String>,
    pub scaler_mean: Vec<f64>,
    pub scaler_std: Vec<f64>,
    pub window_sec: f64,
    pub hop_sec: f64,
    pub forest: Forest,
}

pub fn save_model(model: &ModelFile, path: &Path) -> Result<()> {
    let bytes = bincode::serialize(model).context("模型序列化失败")?;
    let mut file = fs::File::create(path)?;
    file.write_all(MODEL_MAGIC)?;
    file.write_all(&MODEL_VERSION.to_le_bytes())?;
    file.write_all(&bytes)?;
    Ok(())
}

pub fn load_model(path: &Path) -> Result<ModelFile> {
    let mut file = fs::File::open(path).with_context(|| format!("无法打开模型文件: {}", path.display()))?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).context("模型文件头不完整")?;
    if magic != *MODEL_MAGIC { bail!("不是有效的 MP3CutAd 模型文件"); }
    let mut ver_buf = [0u8; 4];
    file.read_exact(&mut ver_buf).context("模型版本字段缺失")?;
    let version = u32::from_le_bytes(ver_buf);
    if version != MODEL_VERSION { bail!("模型版本不兼容: 文件v{}, 当前v{}", version, MODEL_VERSION); }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).context("模型数据读取失败")?;
    let model: ModelFile = bincode::deserialize(&buf).context("模型反序列化失败")?;
    if model.feature_dim != SEG_FEATURE_DIM { bail!("特征维度异常: 期望 {}, 实际 {}", SEG_FEATURE_DIM, model.feature_dim); }
    if model.labels.is_empty() || model.labels.len() > MAX_LABELS { bail!("标签数量异常"); }
    if model.scaler_mean.len() != model.feature_dim || model.scaler_std.len() != model.feature_dim { bail!("Scaler 维度不匹配"); }
    if model.forest.trees.is_empty() || model.forest.trees.len() > MAX_TREES { bail!("森林树数量异常"); }
    // 校验每棵树节点数 > 0（损坏模型的防御）
    for (i, t) in model.forest.trees.iter().enumerate() {
        if t.is_empty() { bail!("森林第 {} 棵树节点为空（模型可能已损坏）", i); }
    }
    let total: usize = model.forest.trees.iter().map(|t| t.len()).sum();
    if total == 0 || total > MAX_NODES { bail!("森林节点总数异常"); }
    if model.forest.n_classes != model.labels.len() { bail!("类别数与标签数不匹配"); }
    if model.window_sec <= 0.0 || model.window_sec > 60.0 { bail!("窗口大小异常"); }
    if model.hop_sec <= 0.0 || model.hop_sec > model.window_sec { bail!("窗口步长异常"); }
    // hop_sec * 采样率必须 >= 1 才不会在分类时取整为 0 导致除零
    if (model.hop_sec * SEG_SR as f64) < 1.0 {
        bail!("窗口步长过小: hop_sec={} 导致 hop_sec*SR < 1 会引发除零", model.hop_sec);
    }
    Ok(model)
}

impl ModelFile {
    /// 标准化特征向量。维度不一致时 panic（由 load_model 校验保证）。
    pub fn scale(&self, feat: &[f64]) -> Vec<f64> {
        assert_eq!(feat.len(), self.scaler_mean.len(),
            "特征维度与 Scaler 不匹配: 期望 {}, 实际 {}", self.scaler_mean.len(), feat.len());
        assert_eq!(self.scaler_mean.len(), self.scaler_std.len(),
            "Scaler mean/std 长度不一致");
        feat.iter()
            .zip(self.scaler_mean.iter().zip(self.scaler_std.iter()))
            .map(|(&v, (&m, &s))| {
                let c = v - m;
                if s < 1e-12 { c } else { c / s }
            })
            .collect()
    }
}
