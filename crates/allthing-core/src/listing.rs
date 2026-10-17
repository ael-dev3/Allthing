// SPDX-License-Identifier: AGPL-3.0-or-later
//! Paging through a ranked pool of casts.

use crate::model::Cast;

pub const PAGE_SIZE: usize = 25;
const MAX_PAGE: u32 = 50;

pub fn parse_page(value: Option<&str>) -> u32 {
    value.and_then(|v| v.trim().parse::<u32>().ok()).filter(|page| *page >= 1).map_or(1, |page| page.min(MAX_PAGE))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageSlice {
    pub page: u32,
    /// Items before this page, so numbering continues across pages.
    pub offset: usize,
    pub end: usize,
    pub has_prev: bool,
    pub has_next: bool,
}

pub fn paginate(len: usize, page: u32, size: usize) -> PageSlice {
    let offset = (page.max(1) as usize - 1) * size;
    PageSlice {
        page,
        offset: offset.min(len),
        end: (offset + size).min(len),
        has_prev: page > 1,
        has_next: offset + size < len,
    }
}

/// Listings show posts, not replies.
pub fn top_level(casts: &[Cast]) -> Vec<Cast> {
    casts.iter().filter(|cast| !cast.is_reply()).cloned().collect()
}

pub fn replies_only(casts: &[Cast]) -> Vec<Cast> {
    casts.iter().filter(|cast| cast.is_reply()).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pages() {
        assert_eq!(parse_page(Some("2")), 2);
        assert_eq!(parse_page(Some("0")), 1);
        assert_eq!(parse_page(Some("x")), 1);
        assert_eq!(parse_page(Some("999")), 50);
        assert_eq!(parse_page(None), 1);
    }

    #[test]
    fn paginates() {
        let first = paginate(60, 1, 25);
        assert_eq!((first.offset, first.end, first.has_prev, first.has_next), (0, 25, false, true));
        let last = paginate(60, 3, 25);
        assert_eq!((last.offset, last.end, last.has_prev, last.has_next), (50, 60, true, false));
        let beyond = paginate(10, 4, 25);
        assert_eq!((beyond.offset, beyond.end), (10, 10));
    }
}
