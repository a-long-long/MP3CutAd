
use ndarray::Array2;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForestParams {
    pub n_trees: usize,
    pub max_depth: usize,
    pub min_samples_split: usize,
    pub max_features: usize,
    pub seed: u64,
}

impl Default for ForestParams {
    fn default() -> Self {
        Self { n_trees: 100, max_depth: 32, min_samples_split: 2, max_features: 0, seed: 42 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub feature: u32,
    pub threshold: f64,
    pub left: u32,
    pub right: u32,
    pub proba: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Forest {
    pub trees: Vec<Vec<Node>>,
    pub n_classes: usize,
    pub feature_dim: usize,
}

impl Forest {
    pub fn train(x: &Array2<f64>, y: &[usize], params: &ForestParams, seed: u64) -> Self {
        let n_samples = x.nrows();
        let dim = x.ncols();
        let n_classes = *y.iter().max().unwrap_or(&0) + 1;
        let mf = if params.max_features == 0 { (dim as f64).sqrt() as usize } else { params.max_features.min(dim) };
        let indices: Vec<usize> = (0..n_samples).collect();
        let trees: Vec<Vec<Node>> = (0..params.n_trees).into_par_iter().map(|t| {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed ^ (t as u64 + 1));
            let mut boot = Vec::with_capacity(n_samples);
            for _ in 0..n_samples { boot.push(*indices.choose(&mut rng).unwrap()); }
            let bx: Vec<Vec<f64>> = boot.iter().map(|&i| x.row(i).to_vec()).collect();
            let by: Vec<usize> = boot.iter().map(|&i| y[i]).collect();
            let mut tree = Vec::new();
            build_tree(&bx, &by, 0, params.max_depth, params.min_samples_split, mf, n_classes, &mut tree, &mut rng);
            tree
        }).collect();
        Forest { trees, n_classes, feature_dim: dim }
    }
    pub fn predict(&self, feat: &[f64]) -> (usize, f64) {
        let p = self.predict_proba(feat);
        p.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, c)| (i, *c)).unwrap_or((0, 0.0))
    }
    pub fn predict_proba(&self, feat: &[f64]) -> Vec<f64> {
        let mut s = vec![0.0f64; self.n_classes];
        for t in &self.trees { let n = traverse(t, feat); for (i, &p) in n.proba.iter().enumerate() { if i < s.len() { s[i] += p; } } }
        let n = self.trees.len().max(1) as f64;
        for v in s.iter_mut() { *v /= n; }
        s
    }
}

fn traverse<'a>(tree: &'a [Node], feat: &[f64]) -> &'a Node {
    let mut i = tree.len() - 1;
    loop {
        let n = &tree[i];
        if n.feature == u32::MAX { return n; }
        let f = n.feature as usize;
        if f >= feat.len() { return n; }
        i = if feat[f] <= n.threshold { n.left as usize } else { n.right as usize };
    }
}

fn build_tree<R: rand::Rng>(x: &[Vec<f64>], y: &[usize], depth: usize, max_depth: usize, min_split: usize, mf: usize, nc: usize, out: &mut Vec<Node>, rng: &mut R) -> u32 {
    let n = x.len();
    let pure = n > 0 && y.iter().all(|&v| v == y[0]);
    if depth >= max_depth || n < min_split || pure {
        let idx = out.len() as u32;
        out.push(Node { feature: u32::MAX, threshold: 0.0, left: 0, right: 0, proba: class_proba(y, nc) });
        return idx;
    }
    let dim = x[0].len();
    let mut feats: Vec<usize> = (0..dim).collect();
    feats.shuffle(rng); feats.truncate(mf);
    let mut bs = f64::MAX; let mut bf = 0u32; let mut bt = 0.0f64; let mut bl: Vec<usize> = Vec::new(); let mut br: Vec<usize> = Vec::new();
    let pg = gini(y, nc);
    for &f in &feats {
        let mut cv: Vec<(f64, usize)> = x.iter().enumerate().map(|(i, r)| (r[f], i)).collect();
        cv.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut thrs = Vec::new();
        for i in 1..cv.len() { if (cv[i].0 - cv[i-1].0).abs() > 1e-12 { thrs.push((cv[i].0 + cv[i-1].0) / 2.0); } }
        if thrs.is_empty() { continue; }
        if thrs.len() > 16 { let s = thrs.len() / 16; thrs = thrs.into_iter().step_by(s.max(1)).collect(); }
        for &t in &thrs {
            let mut li = Vec::new(); let mut ri = Vec::new();
            for &(v, i) in &cv { if v <= t { li.push(i); } else { ri.push(i); } }
            if li.is_empty() || ri.is_empty() { continue; }
            let gl = gini(&iy(y, &li), nc); let gr = gini(&iy(y, &ri), nc);
            let sc = pg - (li.len() as f64 / n as f64) * gl - (ri.len() as f64 / n as f64) * gr;
            if sc < bs { bs = sc; bf = f as u32; bt = t; bl = li.clone(); br = ri.clone(); }
        }
    }
    if bs >= f64::MAX {
        let idx = out.len() as u32;
        out.push(Node { feature: u32::MAX, threshold: 0.0, left: 0, right: 0, proba: class_proba(y, nc) });
        return idx;
    }
    let lx = ix(x, &bl); let ly = iy(y, &bl); let rx = ix(x, &br); let ry = iy(y, &br);
    let lc = build_tree(&lx, &ly, depth+1, max_depth, min_split, mf, nc, out, rng);
    let rc = build_tree(&rx, &ry, depth+1, max_depth, min_split, mf, nc, out, rng);
    let idx = out.len() as u32;
    out.push(Node { feature: bf, threshold: bt, left: lc, right: rc, proba: Vec::new() });
    idx
}
fn iy(y: &[usize], i: &[usize]) -> Vec<usize> { i.iter().map(|&k| y[k]).collect() }
fn ix(x: &[Vec<f64>], i: &[usize]) -> Vec<Vec<f64>> { i.iter().map(|&k| x[k].clone()).collect() }
fn class_proba(y: &[usize], nc: usize) -> Vec<f64> {
    let mut c = vec![0usize; nc];
    for &v in y { if v < nc { c[v] += 1; } }
    let n = y.len().max(1) as f64;
    c.into_iter().map(|v| v as f64 / n).collect()
}
fn gini(y: &[usize], nc: usize) -> f64 {
    let p = class_proba(y, nc);
    1.0 - p.iter().map(|x| x * x).sum::<f64>()
}
