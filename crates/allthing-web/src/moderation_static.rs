// SPDX-License-Identifier: AGPL-3.0-or-later
//! The public static copy has no sign-in or private account preferences.
//! Keep shared rendering boundaries explicit without including the private
//! connection, IndexedDB outbox, or account controls in that build.

use std::collections::BTreeMap;

use allthing_core::model::{Cast, UserSummary};
use allthing_core::moderation::References;
use leptos::prelude::*;

#[derive(Clone, Copy)]
pub struct Moderation;
impl Moderation {
    pub fn new() -> Self {
        Self
    }
}

pub fn waiting() -> bool {
    false
}
pub fn hides_cast(_cast: &Cast, _quotes: &BTreeMap<String, Cast>) -> bool {
    false
}
pub fn hides_user(_user: &UserSummary) -> bool {
    false
}
pub fn hides_references(_references: &References) -> bool {
    false
}

#[component]
pub fn FilterNotice() -> impl IntoView {}
