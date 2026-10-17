// SPDX-License-Identifier: AGPL-3.0-or-later
//! Spam labels published by the main Farcaster client (github.com/warpcast/labels).
//!
//! The dataset is a ~125 MB JSON-lines file, far too big to fetch per profile
//! view. At build time it is compacted into fixed-size shards of one
//! character per fid, served as free static assets. A profile view fetches
//! one ~32 KB shard (a few KB compressed) and reads one character.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Fids per shard file.
pub const SHARD_SIZE: u64 = 32_768;
/// Character for a fid the dataset has no label for.
pub const UNKNOWN: u8 = b'-';

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpamLabel {
    /// Label value 0: "Likely to engage in spammy behavior."
    LikelySpam,
    /// Label value 2: "Unlikely to engage in spammy behavior."
    NotSpam,
    /// Label value 3: "Nerfed for malicious activity."
    Nerfed,
}

impl SpamLabel {
    pub fn from_value(value: u8) -> Option<SpamLabel> {
        match value {
            0 => Some(SpamLabel::LikelySpam),
            2 => Some(SpamLabel::NotSpam),
            3 => Some(SpamLabel::Nerfed),
            _ => None,
        }
    }

    pub fn code(self) -> u8 {
        match self {
            SpamLabel::LikelySpam => b'0',
            SpamLabel::NotSpam => b'2',
            SpamLabel::Nerfed => b'3',
        }
    }

    pub fn from_code(code: u8) -> Option<SpamLabel> {
        match code {
            b'0' => Some(SpamLabel::LikelySpam),
            b'2' => Some(SpamLabel::NotSpam),
            b'3' => Some(SpamLabel::Nerfed),
            _ => None,
        }
    }

    /// Whether the main client restricts the account (shown as a warning).
    pub fn is_flag(self) -> bool {
        !matches!(self, SpamLabel::NotSpam)
    }

    pub fn title(self) -> &'static str {
        match self {
            SpamLabel::LikelySpam => "Labeled likely spam",
            SpamLabel::NotSpam => "Not labeled spam",
            SpamLabel::Nerfed => "Nerfed for malicious activity",
        }
    }

    /// The dataset's own description of the label.
    pub fn description(self) -> &'static str {
        match self {
            SpamLabel::LikelySpam => "Likely to engage in spammy behavior.",
            SpamLabel::NotSpam => "Unlikely to engage in spammy behavior.",
            SpamLabel::Nerfed => "Nerfed for malicious activity.",
        }
    }
}

pub fn shard_of(fid: u64) -> u64 {
    fid / SHARD_SIZE
}

/// Static asset path of a fid's shard, relative to the site root.
pub fn shard_path(fid: u64) -> String {
    format!("/data/labels/spam/{:04}.txt", shard_of(fid))
}

/// Read a fid's label from its shard's contents.
pub fn lookup(shard: &str, fid: u64) -> Option<SpamLabel> {
    let offset = (fid % SHARD_SIZE) as usize;
    shard.as_bytes().get(offset).copied().and_then(SpamLabel::from_code)
}

#[derive(Debug, Deserialize)]
struct LineTarget {
    fid: u64,
}

#[derive(Debug, Deserialize)]
struct Line {
    #[serde(rename = "type")]
    target: LineTarget,
    label_type: String,
    label_value: u8,
    #[serde(default)]
    timestamp: i64,
}

/// (fid, label, timestamp) from one dataset line, or `None` for other label
/// types, unknown values and malformed lines.
pub fn parse_line(line: &str) -> Option<(u64, SpamLabel, i64)> {
    let line: Line = serde_json::from_str(line).ok()?;
    if line.label_type != "spam" {
        return None;
    }
    Some((line.target.fid, SpamLabel::from_value(line.label_value)?, line.timestamp))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelCounts {
    pub likely_spam: u64,
    pub not_spam: u64,
    pub nerfed: u64,
}

/// `/data/labels/spam/meta.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelsMeta {
    pub source: String,
    /// Upstream dataset version (git LFS object id or commit), if known.
    #[serde(default)]
    pub version: Option<String>,
    /// Unix seconds of the newest label in the dataset.
    #[serde(default)]
    pub newest_label: Option<i64>,
    /// Unix seconds when the shards were built.
    pub built_at: i64,
    pub shard_size: u64,
    pub shards: u64,
    pub counts: LabelCounts,
}

/// Accumulates dataset lines (newest label per fid wins) into shards.
#[derive(Debug, Default)]
pub struct ShardBuilder {
    labels: BTreeMap<u64, (SpamLabel, i64)>,
}

impl ShardBuilder {
    pub fn add(&mut self, fid: u64, label: SpamLabel, ts: i64) {
        let entry = self.labels.entry(fid).or_insert((label, ts));
        if ts >= entry.1 {
            *entry = (label, ts);
        }
    }

    pub fn add_line(&mut self, line: &str) -> bool {
        match parse_line(line) {
            Some((fid, label, ts)) => {
                self.add(fid, label, ts);
                true
            }
            None => false,
        }
    }

    pub fn counts(&self) -> LabelCounts {
        let mut counts = LabelCounts::default();
        for (label, _) in self.labels.values() {
            match label {
                SpamLabel::LikelySpam => counts.likely_spam += 1,
                SpamLabel::NotSpam => counts.not_spam += 1,
                SpamLabel::Nerfed => counts.nerfed += 1,
            }
        }
        counts
    }

    pub fn newest(&self) -> Option<i64> {
        self.labels.values().map(|(_, ts)| *ts).max()
    }

    /// Shard number -> contents. Every shard up to the highest labeled fid
    /// exists, so a missing file always means "beyond the dataset".
    pub fn shards(&self) -> BTreeMap<u64, String> {
        let Some(max_fid) = self.labels.keys().next_back() else {
            return BTreeMap::new();
        };
        let mut out: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
        for shard in 0..=shard_of(*max_fid) {
            out.insert(shard, vec![UNKNOWN; SHARD_SIZE as usize]);
        }
        for (fid, (label, _)) in &self.labels {
            if let Some(bytes) = out.get_mut(&shard_of(*fid)) {
                bytes[(fid % SHARD_SIZE) as usize] = label.code();
            }
        }
        out.into_iter().map(|(k, v)| (k, String::from_utf8(v).unwrap_or_default())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dataset_lines() {
        let line = r#"{"provider": 9152, "type": {"target": "user", "fid": 481092}, "label_type": "spam", "label_value": 0, "timestamp": 1737596669}"#;
        assert_eq!(parse_line(line), Some((481_092, SpamLabel::LikelySpam, 1_737_596_669)));
        assert_eq!(
            parse_line(&line.replace("\"label_value\": 0", "\"label_value\": 3")).map(|l| l.1),
            Some(SpamLabel::Nerfed)
        );
        assert_eq!(parse_line(&line.replace("\"label_value\": 0", "\"label_value\": 1")), None);
        assert_eq!(parse_line(&line.replace("\"spam\"", "\"other\"")), None);
        assert_eq!(parse_line("not json"), None);
    }

    #[test]
    fn builds_and_reads_shards() {
        let mut builder = ShardBuilder::default();
        builder.add(3, SpamLabel::NotSpam, 10);
        builder.add(3, SpamLabel::LikelySpam, 5); // older, ignored
        builder.add(40_000, SpamLabel::Nerfed, 20);
        builder.add(40_001, SpamLabel::LikelySpam, 20);
        let shards = builder.shards();
        assert_eq!(shards.len(), 2);
        assert!(shards.values().all(|s| s.len() == SHARD_SIZE as usize));
        assert_eq!(lookup(&shards[&0], 3), Some(SpamLabel::NotSpam));
        assert_eq!(lookup(&shards[&0], 4), None);
        assert_eq!(lookup(&shards[&1], 40_000), Some(SpamLabel::Nerfed));
        assert_eq!(lookup(&shards[&1], 40_001), Some(SpamLabel::LikelySpam));
        assert_eq!(builder.counts(), LabelCounts { likely_spam: 1, not_spam: 1, nerfed: 1 });
        assert_eq!(builder.newest(), Some(20));
        assert_eq!(shard_path(40_000), "/data/labels/spam/0001.txt");
        assert!(SpamLabel::Nerfed.is_flag() && !SpamLabel::NotSpam.is_flag());
    }
}
