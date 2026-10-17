//! Entirely fictional content for exercising the seed without accessing real accounts.

use crate::{Comment, Network, Post, SourceMetrics};

pub const CHANNELS: [&str; 6] = ["dev", "design", "gaming", "music", "science", "books"];

pub fn posts() -> Vec<Post> {
    vec![
        Post {
            id: "demo-reader".into(),
            title: "A small Rust reader for a very big conversation".into(),
            body: "Trying an interface with fewer distractions: a chronological feed, readable threads, and the choice of where your data comes from. What would you keep from the early web?".into(),
            author: "sample_fern".into(),
            channel: "dev".into(),
            age: "12m".into(),
            published_order: 6,
            metrics: vec![metric(Network::Snapchain, 42, 7), metric(Network::Hypersnap, 45, 8)],
            comments: vec![
                comment("reader-a", "sample_cedar", "Dense lists and threads that remember where I was reading. Give the words room, not the buttons.", "8m", vec![
                    comment("reader-a-1", "sample_fern", "Yes. A collapsed thread should be one click away from coming back.", "5m", vec![]),
                ]),
                comment("reader-b", "sample_moss", "Please keep the source labels visible. I want to understand which view of the conversation I am seeing.", "4m", vec![]),
            ],
        },
        Post {
            id: "demo-gardens".into(),
            title: "The internet could use more little gardens".into(),
            body: "Personal sites, small communities, and useful things made with care. A good corner of the web does not need to contain the entire web.".into(),
            author: "sample_cedar".into(),
            channel: "design".into(),
            age: "28m".into(),
            published_order: 5,
            metrics: vec![metric(Network::Hypersnap, 86, 12)],
            comments: vec![comment("gardens-a", "sample_linden", "The best communities feel small even when the network around them is large.", "20m", vec![])],
        },
        Post {
            id: "demo-factory".into(),
            title: "Show us the wonderfully impractical thing you built this week".into(),
            body: "Mine is a factory with a train that travels farther than the ore ever needed to go. It is not efficient. It is very satisfying.".into(),
            author: "sample_moss".into(),
            channel: "gaming".into(),
            age: "46m".into(),
            published_order: 4,
            metrics: vec![metric(Network::Snapchain, 31, 4)],
            comments: vec![comment("factory-a", "sample_birch", "A bridge that exists purely because I wanted a bridge.", "35m", vec![])],
        },
        Post {
            id: "demo-record".into(),
            title: "An album you always listen to from beginning to end?".into(),
            body: "Looking for records that feel like a single journey. Any language, any decade. Tell me what makes the whole thing work.".into(),
            author: "sample_willow".into(),
            channel: "music".into(),
            age: "1h".into(),
            published_order: 3,
            metrics: vec![metric(Network::Snapchain, 58, 9), metric(Network::Hypersnap, 58, 9)],
            comments: vec![],
        },
        Post {
            id: "demo-sky".into(),
            title: "A notebook, a clear sky, and an hour away from screens".into(),
            body: "Learning the constellations slowly, one small patch of sky at a time. There is something nice about having no progress bar.".into(),
            author: "sample_birch".into(),
            channel: "science".into(),
            age: "2h".into(),
            published_order: 2,
            metrics: vec![metric(Network::Hypersnap, 24, 2)],
            comments: vec![],
        },
        Post {
            id: "demo-books".into(),
            title: "Which fictional place would make a good public library?".into(),
            body: "Bonus points for a quiet reading room, a confusing history, and a journey that takes longer than checking out the book.".into(),
            author: "sample_linden".into(),
            channel: "books".into(),
            age: "3h".into(),
            published_order: 1,
            metrics: vec![metric(Network::Snapchain, 19, 1)],
            comments: vec![],
        },
    ]
}

fn metric(network: Network, likes: u32, recasts: u32) -> SourceMetrics {
    SourceMetrics {
        network,
        likes,
        recasts,
    }
}

fn comment(id: &str, author: &str, body: &str, age: &str, replies: Vec<Comment>) -> Comment {
    Comment {
        id: id.into(),
        author: author.into(),
        body: body.into(),
        age: age.into(),
        replies,
    }
}
