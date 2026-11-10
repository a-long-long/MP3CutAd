//! 检测结果文件的读写
//!
//! 纯文本格式，便于人工查看：
//! ```text
//! <文件数>
//! <文件路径>
//! <总帧数>
//! <区间数>
//! <begin> <end> <count> <type_id> [score]
//! ```
//!
//! `score` 为匹配度，旧版结果文件没有该字段时按 0 处理。

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::core::detect::DetectionResult;
use crate::core::range::Range;

/// 保存检测结果
pub fn save(result: &DetectionResult, files: &[PathBuf], output: &Path) -> Result<()> {
    if files.len() != result.ranges.len() || files.len() != result.lengths.len() {
        bail!(
            "文件数量与检测结果不匹配: {} 个文件, {} 组区间",
            files.len(),
            result.ranges.len()
        );
    }

    let file = std::fs::File::create(output)
        .with_context(|| format!("无法创建结果文件: {}", output.display()))?;
    let mut writer = BufWriter::new(file);

    writeln!(writer, "{}", files.len())?;

    for (i, path) in files.iter().enumerate() {
        writeln!(writer, "{}", path.display())?;
        writeln!(writer, "{}", result.lengths[i])?;
        writeln!(writer, "{}", result.ranges[i].len())?;

        for r in &result.ranges[i] {
            writeln!(
                writer,
                "{} {} {} {} {:.4}",
                r.begin, r.end, r.count, r.type_id, r.score
            )?;
        }
    }

    writer.flush()?;
    Ok(())
}

/// 读取检测结果
pub fn load(path: &Path) -> Result<DetectionResult> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("无法打开结果文件: {}", path.display()))?;
    let mut lines = BufReader::new(file).lines();

    let num_files: usize = next_line(&mut lines)?
        .parse()
        .context("结果文件头不是有效的文件数量")?;

    let mut ranges = Vec::with_capacity(num_files);
    let mut lengths = Vec::with_capacity(num_files);

    for file_idx in 0..num_files {
        let _path = next_line(&mut lines)?;
        let length: usize = next_line(&mut lines)?
            .parse()
            .with_context(|| format!("第 {} 个文件的总帧数无效", file_idx + 1))?;
        let num_ranges: usize = next_line(&mut lines)?
            .parse()
            .with_context(|| format!("第 {} 个文件的区间数量无效", file_idx + 1))?;

        let mut file_ranges = Vec::with_capacity(num_ranges);
        for range_idx in 0..num_ranges {
            let line = next_line(&mut lines)?;
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 4 {
                bail!(
                    "第 {} 个文件的第 {} 个区间格式无效: {}",
                    file_idx + 1,
                    range_idx + 1,
                    line
                );
            }

            file_ranges.push(Range {
                begin: parts[0].parse().context("区间起始位置无效")?,
                end: parts[1].parse().context("区间结束位置无效")?,
                count: parts[2].parse().context("区间出现次数无效")?,
                type_id: parts[3].parse().context("区间类型无效")?,
                score: parts
                    .get(4)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0),
            });
        }

        ranges.push(file_ranges);
        lengths.push(length);
    }

    Ok(DetectionResult { ranges, lengths })
}

fn next_line(lines: &mut impl Iterator<Item = std::io::Result<String>>) -> Result<String> {
    lines.next().transpose()?.context("结果文件格式不完整")
}
