// SPDX-License-Identifier: AGPL-3.0-or-later
//! The main Farcaster client's spam labels, read from static shards built at
//! deploy time (free static assets; no Worker request).

use allthing_core::labels::{self, LabelsMeta, SpamLabel};

use crate::api::get_text;
use crate::util::href;

#[derive(Clone, Debug, PartialEq)]
pub enum LabelState {
    /// The dataset has a label for this account.
    Labeled(SpamLabel, LabelsMeta),
    /// The dataset has no label (not enough activity, or newer than the dataset).
    Unlabeled(LabelsMeta),
    /// The shards were not built for this deployment.
    Unavailable,
}

pub async fn spam_label(fid: u64) -> LabelState {
    let Some(meta) = get_text(&href("/data/labels/spam/meta.json"))
        .await
        .and_then(|text| serde_json::from_str::<LabelsMeta>(&text).ok())
    else {
        return LabelState::Unavailable;
    };
    if labels::shard_of(fid) >= meta.shards {
        return LabelState::Unlabeled(meta);
    }
    match get_text(&href(&labels::shard_path(fid))).await {
        Some(shard) => match labels::lookup(&shard, fid) {
            Some(label) => LabelState::Labeled(label, meta),
            None => LabelState::Unlabeled(meta),
        },
        None => LabelState::Unavailable,
    }
}
