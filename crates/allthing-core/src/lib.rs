//! Shared, transport-independent view models. No credentials or network access.

pub mod demo;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Network {
    Snapchain,
    Hypersnap,
}

impl Network {
    pub fn label(self) -> &'static str {
        match self {
            Self::Snapchain => "Snapchain",
            Self::Hypersnap => "Hypersnap",
        }
    }

    pub fn class(self) -> &'static str {
        match self {
            Self::Snapchain => "snapchain",
            Self::Hypersnap => "hypersnap",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SourceFilter {
    #[default]
    All,
    Snapchain,
    Hypersnap,
}

impl SourceFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Snapchain, Self::Hypersnap];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All sources",
            Self::Snapchain => "Snapchain",
            Self::Hypersnap => "Hypersnap",
        }
    }

    fn matches(self, post: &Post) -> bool {
        match self {
            Self::All => true,
            Self::Snapchain => post.metrics.iter().any(|m| m.network == Network::Snapchain),
            Self::Hypersnap => post.metrics.iter().any(|m| m.network == Network::Hypersnap),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FeedSort {
    #[default]
    New,
    Top,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceMetrics {
    pub network: Network,
    pub likes: u32,
    pub recasts: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub age: String,
    pub replies: Vec<Comment>,
}

impl Comment {
    pub fn total_count(&self) -> usize {
        1 + self.replies.iter().map(Self::total_count).sum::<usize>()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Post {
    /// A display identifier in this seed. Live adapters must use verified FID/hash identity.
    pub id: String,
    pub title: String,
    pub body: String,
    pub author: String,
    pub channel: String,
    pub age: String,
    pub published_order: i64,
    /// Independent snapshots, never added together across overlapping data sources.
    pub metrics: Vec<SourceMetrics>,
    pub comments: Vec<Comment>,
}

impl Post {
    /// Prefer Snapchain for a consistent display when both sources observe a post.
    /// This is a presentation policy, not a claim that one snapshot is authoritative.
    pub fn display_metrics(&self) -> Option<&SourceMetrics> {
        self.metrics
            .iter()
            .find(|m| m.network == Network::Snapchain)
            .or_else(|| self.metrics.first())
    }

    pub fn comment_count(&self) -> usize {
        self.comments.iter().map(Comment::total_count).sum()
    }
}

#[derive(Clone, Debug, Default)]
pub struct FeedQuery {
    pub source: SourceFilter,
    pub sort: FeedSort,
    pub channel: Option<String>,
    pub search: String,
}

/// Query an already normalized feed. Identity reconciliation belongs in live adapters.
pub fn query_feed(posts: &[Post], query: &FeedQuery) -> Vec<Post> {
    let needle = query.search.trim().to_lowercase();
    let mut result: Vec<_> = posts
        .iter()
        .filter(|post| query.source.matches(post))
        .filter(|post| query.channel.as_ref().is_none_or(|c| c == &post.channel))
        .filter(|post| {
            needle.is_empty()
                || [&post.title, &post.body, &post.author, &post.channel]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&needle))
        })
        .cloned()
        .collect();

    result.sort_by(|a, b| {
        let by_new = b.published_order.cmp(&a.published_order);
        match query.sort {
            FeedSort::New => by_new,
            FeedSort::Top => b
                .display_metrics()
                .map_or(0, |m| m.likes)
                .cmp(&a.display_metrics().map_or(0, |m| m.likes))
                .then(by_new),
        }
        .then_with(|| a.id.cmp(&b.id))
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_post_is_visible_once_in_either_source() {
        let posts = demo::posts();
        for source in SourceFilter::ALL {
            let result = query_feed(
                &posts,
                &FeedQuery {
                    source,
                    ..Default::default()
                },
            );
            assert_eq!(
                result
                    .iter()
                    .filter(|post| post.id == "demo-reader")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn overlapping_metrics_are_not_added_or_selected_by_largest_count() {
        let mut post = demo::posts().remove(0);
        post.metrics = vec![
            SourceMetrics {
                network: Network::Hypersnap,
                likes: 250,
                recasts: 40,
            },
            SourceMetrics {
                network: Network::Snapchain,
                likes: 24,
                recasts: 3,
            },
        ];
        let displayed = post.display_metrics().unwrap();
        assert_eq!((displayed.likes, displayed.recasts), (24, 3));
        assert_eq!(post.metrics[0].likes, 250);
    }

    #[test]
    fn query_combines_filters_and_orders_newest_or_most_liked() {
        let posts = demo::posts();
        let dev = query_feed(
            &posts,
            &FeedQuery {
                channel: Some("dev".into()),
                search: " RUST ".into(),
                ..Default::default()
            },
        );
        assert_eq!(dev.len(), 1);
        assert_eq!(dev[0].id, "demo-reader");
        assert_eq!(
            query_feed(&posts, &FeedQuery::default())[0].id,
            "demo-reader"
        );
        assert_eq!(
            query_feed(
                &posts,
                &FeedQuery {
                    sort: FeedSort::Top,
                    ..Default::default()
                }
            )[0]
            .id,
            "demo-gardens"
        );
    }
}
