// SPDX-License-Identifier: AGPL-3.0-or-later
//! Which apps casts are posted from. A cast's app is the one that requested
//! its signing key (its provenance); a list's shares count its casts by app.

use std::collections::BTreeMap;

use crate::api::Annotations;
use crate::model::{Cast, UserSummary};

/// The Farcaster app's account. It is still named Warpcast, the app's name
/// until 2025.
pub const FARCASTER_APP_FID: u64 = 9152;

/// The account Allthing asks for readers' keys as (`ALLTHING_FID`): its own,
/// @allthingapp. Casts posted with those keys are Allthing's.
pub const ALLTHING_APP_FID: u64 = 3_356_318;

/// What to call the app casts were posted from.
pub fn client_name(app: &UserSummary) -> &str {
    match app.fid {
        FARCASTER_APP_FID => "Farcaster",
        ALLTHING_APP_FID => "Allthing",
        _ => app.name(),
    }
}

/// The profile key (as `/u/<key>` takes it) for the app's account: its fid
/// for the Farcaster app, whose account still has the old "warpcast" name.
pub fn client_profile(app: &UserSummary) -> String {
    if app.fid == FARCASTER_APP_FID { format!("fid:{}", app.fid) } else { app.username.clone() }
}

/// One app's share of a list's casts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientShare {
    /// The app's account; `None` for the remaining apps together.
    pub app: Option<UserSummary>,
    /// The app's fid, `None` for the remaining apps together.
    pub fid: Option<u64>,
    pub name: String,
    pub casts: u64,
    /// Whole percent of all the list's casts; with the unknown's, a list's
    /// shares sum to 100.
    pub percent: u32,
}

/// A list's casts by the app they were posted from, each a whole percent of
/// all of them: the apps with the most casts, the rest of the known apps
/// together ("Other apps"), and the casts whose app isn't known, which sum
/// to 100. While most apps are still being read, the known ones' shares are
/// of all the casts, not of the few known, which would overstate them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientShares {
    /// The apps by name, most casts first, then "Other apps" when there are more.
    pub shares: Vec<ClientShare>,
    /// Casts whose app isn't known: still being read (`Annotations::pending`),
    /// or not to be had.
    pub unknown: u64,
    /// Their whole percent of all the casts.
    pub unknown_percent: u32,
}

impl ClientShares {
    /// The casts whose app is known.
    pub fn known(&self) -> u64 {
        self.shares.iter().map(|s| s.casts).sum()
    }
}

/// Whole percents of `counts` that sum to 100, by the largest-remainder
/// method (all zero when the counts are).
fn percents(counts: &[u64]) -> Vec<u32> {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return vec![0; counts.len()];
    }
    let mut out: Vec<u32> = counts.iter().map(|n| (n * 100 / total) as u32).collect();
    let mut by_remainder: Vec<usize> = (0..counts.len()).collect();
    by_remainder.sort_by(|&a, &b| ((counts[b] * 100) % total).cmp(&((counts[a] * 100) % total)).then(a.cmp(&b)));
    let short = 100 - out.iter().sum::<u32>();
    for &i in by_remainder.iter().take(short as usize) {
        out[i] += 1;
    }
    out
}

/// [`ClientShares`] of `casts` by posting app, from the origins in `notes`:
/// the `top` biggest apps by casts, then the rest as "Other apps", then the
/// unknown. `None` for no casts, or while no cast's app is known and none is
/// being read (as in the static copy, which reads no origins).
pub fn client_shares(casts: &[Cast], notes: &Annotations, top: usize) -> Option<ClientShares> {
    let mut counts: BTreeMap<u64, u64> = BTreeMap::new();
    for fid in casts.iter().filter_map(|c| notes.origins.get(&c.hash)?.app_fid) {
        *counts.entry(fid).or_default() += 1;
    }
    let known: u64 = counts.values().sum();
    if casts.is_empty() || (known == 0 && notes.pending == 0) {
        return None;
    }
    let unknown = (casts.len() as u64).saturating_sub(known);
    let mut apps: Vec<(u64, u64)> = counts.into_iter().collect();
    apps.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let rest: u64 = apps.iter().skip(top).map(|(_, n)| n).sum();
    let mut shares: Vec<ClientShare> = apps
        .iter()
        .take(top)
        .map(|&(fid, casts)| {
            let app = notes.apps.get(&fid).cloned();
            let name = app.as_ref().map_or_else(|| format!("app fid {fid}"), |a| client_name(a).to_owned());
            ClientShare { app, fid: Some(fid), name, casts, percent: 0 }
        })
        .collect();
    if rest > 0 {
        shares.push(ClientShare { app: None, fid: None, name: "Other apps".into(), casts: rest, percent: 0 });
    }
    // Each a share of all the casts, the unknown last.
    let mut split = percents(&shares.iter().map(|s| s.casts).chain([unknown]).collect::<Vec<_>>());
    let unknown_percent = split.pop().unwrap_or(0);
    for (share, percent) in shares.iter_mut().zip(split) {
        share.percent = percent;
    }
    Some(ClientShares { shares, unknown, unknown_percent })
}

/// The apps past the first `top` of `casts`' shares, each with its own
/// share of the known casts, largest first: what "Other apps" holds.
pub fn other_clients(casts: &[Cast], notes: &Annotations, top: usize) -> Vec<ClientShare> {
    let mut counts: BTreeMap<u64, u64> = BTreeMap::new();
    for fid in casts.iter().filter_map(|c| notes.origins.get(&c.hash)?.app_fid) {
        *counts.entry(fid).or_default() += 1;
    }
    let mut apps: Vec<(u64, u64)> = counts.into_iter().collect();
    apps.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let rest: Vec<(u64, u64)> = apps.into_iter().skip(top).collect();
    let split = percents(&rest.iter().map(|&(_, n)| n).collect::<Vec<_>>());
    rest.into_iter()
        .zip(split)
        .map(|((fid, casts), percent)| {
            let app = notes.apps.get(&fid).cloned();
            let name = app.as_ref().map_or_else(|| format!("app fid {fid}"), |a| client_name(a).to_owned());
            ClientShare { app, fid: Some(fid), name, casts, percent }
        })
        .collect()
}

/// The casts posted from app `fid`, in their order.
pub fn from_client(casts: Vec<Cast>, notes: &Annotations, fid: u64) -> Vec<Cast> {
    casts.into_iter().filter(|c| notes.origins.get(&c.hash).and_then(|o| o.app_fid) == Some(fid)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Origin;

    #[test]
    fn the_farcaster_app_is_linked_by_fid_not_its_old_name() {
        let app =
            UserSummary { fid: 9152, username: "warpcast".into(), display_name: "Warpcast".into(), pfp_url: None };
        assert_eq!(client_profile(&app), "fid:9152");
        let other = UserSummary { fid: 309857, username: "baseapp".into(), display_name: String::new(), pfp_url: None };
        assert_eq!(client_profile(&other), "baseapp");
    }

    fn app(fid: u64, username: &str, display_name: &str) -> UserSummary {
        UserSummary { fid, username: username.into(), display_name: display_name.into(), pfp_url: None }
    }

    fn cast(hash: &str) -> Cast {
        Cast {
            hash: hash.into(),
            author: app(1, "a", ""),
            text: String::new(),
            mentions: vec![],
            ts: 0,
            parent_hash: None,
            parent_url: None,
            channel: None,
            embeds: vec![],
            likes: 0,
            recasts: 0,
            replies: 0,
            mini_app: false,
            spam: false,
        }
    }

    #[test]
    fn the_farcaster_apps_account_is_called_farcaster() {
        assert_eq!(client_name(&app(9152, "warpcast", "Warpcast")), "Farcaster");
        assert_eq!(client_name(&app(309857, "baseapp.base.eth", "Base App")), "Base App");
        assert_eq!(client_name(&app(42, "someapp", "")), "someapp");
    }

    #[test]
    fn casts_from_keys_allthing_asked_for_are_allthings() {
        assert_eq!(client_name(&app(ALLTHING_APP_FID, "allthingapp", "Allthing")), "Allthing");
        // Its account is the owner's profile.
        assert_eq!(client_profile(&app(ALLTHING_APP_FID, "allthingapp", "Allthing")), "allthingapp");
    }

    #[test]
    fn other_clients_lists_the_apps_past_the_top_and_a_filter_keeps_one_app() {
        let casts: Vec<Cast> = (0..11).map(|i| cast(&format!("0x{i}"))).collect();
        let mut notes = Annotations::default();
        let apps = [9152, 9152, 9152, 9152, 9152, 309857, 309857, 309857, 7, 8];
        for (i, fid) in apps.iter().enumerate() {
            notes.origins.insert(format!("0x{i}"), Origin { signer: "0x01".into(), app_fid: Some(*fid) });
        }
        notes.apps.insert(9152, app(9152, "warpcast", "Warpcast"));
        // Top two are 9152 and 309857; the rest are 7 and 8, one cast each.
        let rest = other_clients(&casts, &notes, 2);
        assert_eq!(rest.iter().map(|s| s.fid).collect::<Vec<_>>(), [Some(7), Some(8)]);
        assert_eq!(rest.iter().map(|s| s.casts).collect::<Vec<_>>(), [1, 1]);
        assert_eq!(rest[0].name, "app fid 7");
        // Filtering keeps only that app's casts, in order.
        let only = from_client(casts.clone(), &notes, 309857);
        assert_eq!(only.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(), ["0x5", "0x6", "0x7"]);
        assert!(from_client(casts, &notes, 404).is_empty());
    }

    #[test]
    fn percents_sum_to_a_hundred() {
        assert_eq!(percents(&[1, 1, 1]), [34, 33, 33]);
        assert_eq!(percents(&[15, 6, 4]), [60, 24, 16]);
        assert_eq!(percents(&[0, 0]), [0, 0]);
    }

    #[test]
    fn shares_count_casts_by_posting_app_with_the_rest_together() {
        let casts: Vec<Cast> = (0..12).map(|i| cast(&format!("0x{i}"))).collect();
        let mut notes = Annotations::default();
        // 6 from Farcaster, 3 from Base App, 1 each from two others, 1
        // unattributed, 0 unknown for the last.
        let apps = [9152, 9152, 9152, 9152, 9152, 9152, 309857, 309857, 309857, 7, 8];
        for (i, fid) in apps.iter().enumerate() {
            notes.origins.insert(format!("0x{i}"), Origin { signer: "0x01".into(), app_fid: Some(*fid) });
        }
        notes.origins.insert("0x11".into(), Origin { signer: "0x02".into(), app_fid: None });
        notes.apps.insert(9152, app(9152, "warpcast", "Warpcast"));
        notes.apps.insert(309857, app(309857, "baseapp.base.eth", "Base App"));
        let split = client_shares(&casts, &notes, 2).unwrap();
        let summary: Vec<(&str, u64, u32)> =
            split.shares.iter().map(|s| (s.name.as_str(), s.casts, s.percent)).collect();
        // Shares of all 12, the unattributed one among the unknown.
        assert_eq!(summary, [("Farcaster", 6, 50), ("Base App", 3, 25), ("Other apps", 2, 17)]);
        assert_eq!((split.unknown, split.unknown_percent, split.known()), (1, 8, 11));
        // An app whose account isn't known yet goes by its fid.
        let three = client_shares(&casts, &notes, 3).unwrap();
        assert_eq!(three.shares[2].name, "app fid 7");
    }

    #[test]
    fn while_most_apps_are_being_read_the_known_are_shares_of_all_the_casts() {
        // 92 casts: 14 known (10 from Farcaster, 4 from Base App), 78 being read.
        let casts: Vec<Cast> = (0..92).map(|i| cast(&format!("0x{i}"))).collect();
        let mut notes = Annotations { pending: 78, ..Default::default() };
        for i in 0..14 {
            let fid = if i < 10 { 9152 } else { 309857 };
            notes.origins.insert(format!("0x{i}"), Origin { signer: "0x01".into(), app_fid: Some(fid) });
        }
        notes.apps.insert(9152, app(9152, "warpcast", "Warpcast"));
        let split = client_shares(&casts, &notes, 4).unwrap();
        let summary: Vec<(&str, u32)> = split.shares.iter().map(|s| (s.name.as_str(), s.percent)).collect();
        // Not "Farcaster 71%": 10 of 92.
        assert_eq!(summary, [("Farcaster", 11), ("app fid 309857", 4)]);
        assert_eq!((split.unknown, split.unknown_percent), (78, 85));
        assert_eq!(summary.iter().map(|(_, p)| p).sum::<u32>() + split.unknown_percent, 100);
        // None known yet, all on their way: the bar is all unknown.
        let waiting = Annotations { pending: 92, ..Default::default() };
        let none = client_shares(&casts, &waiting, 4).unwrap();
        assert_eq!((none.shares.len(), none.unknown, none.unknown_percent), (0, 92, 100));
        // None known and none coming (the static copy), or no casts: no bar.
        assert_eq!(client_shares(&casts, &Annotations::default(), 4), None);
        assert_eq!(client_shares(&[], &notes, 4), None);
    }

    #[test]
    fn with_every_app_known_the_shares_are_as_before() {
        let casts: Vec<Cast> = (0..25).map(|i| cast(&format!("0x{i}"))).collect();
        let mut notes = Annotations::default();
        for i in 0..25 {
            let fid = match i {
                0..15 => 9152,
                15..21 => 309857,
                _ => 1234,
            };
            notes.origins.insert(format!("0x{i}"), Origin { signer: "0x01".into(), app_fid: Some(fid) });
        }
        let split = client_shares(&casts, &notes, 4).unwrap();
        assert_eq!(split.shares.iter().map(|s| s.percent).collect::<Vec<_>>(), [60, 24, 16]);
        assert_eq!((split.unknown, split.unknown_percent), (0, 0));
    }
}
