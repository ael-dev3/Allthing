use allthing_core::{Comment, FeedQuery, FeedSort, Post, SourceFilter, demo, query_feed};
use leptos::prelude::*;
use leptos_meta::{Meta, MetaTags, Stylesheet, Title, provide_meta_context};
use leptos_router::{
    SsrMode, StaticSegment,
    components::{A, Route, Router, Routes},
};

use crate::feed::get_seed_feed;

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <meta name="color-scheme" content="light"/>
                <link rel="icon" href="/favicon.svg" type="image/svg+xml"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body><App/></body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    view! {
        <Stylesheet id="leptos" href="/pkg/allthing.css"/>
        <Title text="Allthing — a place for the conversation"/>
        <Meta name="description" content="An independent Farcaster client taking shape in Rust. A compact, light-mode home for conversations across Snapchain and Hypersnap."/>
        <Router>
            <a class="skip-link" href="#main">"Skip to content"</a>
            <Header/>
            <Routes fallback=NotFound>
                <Route path=StaticSegment("") view=Home ssr=SsrMode::Async/>
                <Route path=StaticSegment("about") view=About/>
            </Routes>
            <Footer/>
        </Router>
    }
}

#[component]
fn Header() -> impl IntoView {
    view! {
        <div class="topline">
            <span>"an independent corner of the social web"</span>
            <span class="topline-right">"built with Rust · early preview"</span>
        </div>
        <header class="site-header">
            <div class="masthead">
                <A href="/" attr:class="brand" attr:aria-label="Allthing home">
                    <span class="brand-mark" aria-hidden="true">"a"</span>
                    <span>"allthing"<span class="brand-dot">"."</span></span>
                </A>
                <p class="tagline">"a place for the conversation"</p>
            </div>
            <div class="header-bottom">
                <nav class="main-nav" aria-label="Main navigation">
                    <A href="/" exact=true>"front page"</A>
                    <A href="/about">"about allthing"</A>
                </nav>
                <div class="account-area">
                    <span class="light-label">"☀ light mode"</span>
                    <button class="sign-in" disabled title="Sign-in will be added after authentication is integrated">"sign in · soon"</button>
                </div>
            </div>
        </header>
    }
}

#[component]
fn Home() -> impl IntoView {
    let feed = Resource::new(|| (), |_| get_seed_feed());
    view! {
        <main id="main" class="page-layout">
            <section class="feed-column" aria-label="Discussion feed">
                <div class="preview-notice">
                    <span class="notice-tag">"SEED"</span>
                    <p>"Fictional conversations, working interface. Live sources and sign-in are next."</p>
                    <A href="/about">"what we're building →"</A>
                </div>
                <noscript class="noscript-note">"The sample feed is readable here. Enable JavaScript to load WebAssembly for filters and discussion controls."</noscript>
                <Suspense fallback=|| view! { <p class="feed-message" role="status">"Loading conversations…"</p> }>
                    {move || feed.get().map(|result| match result {
                        Ok(posts) => view! { <FeedBoard posts/> }.into_any(),
                        Err(_) => view! {
                            <div class="feed-message" role="alert">
                                <h1>"The sample feed could not be loaded."</h1>
                                <p>"Please reload the page to try again."</p>
                                <a href="/">"Reload feed"</a>
                            </div>
                        }.into_any(),
                    })}
                </Suspense>
            </section>
            <Sidebar/>
        </main>
    }
}

#[component]
fn FeedBoard(posts: Vec<Post>) -> impl IntoView {
    let source = RwSignal::new(SourceFilter::All);
    let sort = RwSignal::new(FeedSort::New);
    let channel = RwSignal::new(None::<String>);
    let search = RwSignal::new(String::new());
    let posts = StoredValue::new(posts);
    let visible = Memo::new(move |_| {
        posts.with_value(|posts| {
            query_feed(
                posts,
                &FeedQuery {
                    source: source.get(),
                    sort: sort.get(),
                    channel: channel.get(),
                    search: search.get(),
                },
            )
        })
    });

    view! {
        <div class="source-toolbar">
            <div class="source-tabs" role="group" aria-label="Data source">
                {SourceFilter::ALL.into_iter().map(|filter| view! {
                    <button
                        class:active=move || source.get() == filter
                        aria-pressed=move || (source.get() == filter).to_string()
                        on:click=move |_| source.set(filter)
                    >{filter.label()}</button>
                }).collect_view()}
            </div>
            <label class="search-box">
                <span class="sr-only">"Search sample posts"</span>
                <span aria-hidden="true">"⌕"</span>
                <input
                    type="search"
                    placeholder="search conversations"
                    maxlength="200"
                    prop:value=move || search.get()
                    on:input=move |event| search.set(event_target_value(&event))
                />
            </label>
        </div>
        <nav class="channel-strip" aria-label="Filter by channel">
            <button class:chosen=move || channel.get().is_none() aria-pressed=move || channel.get().is_none().to_string() on:click=move |_| channel.set(None)>"all channels"</button>
            {demo::CHANNELS.into_iter().map(|name| view! {
                <button
                    class:chosen=move || channel.get().as_deref() == Some(name)
                    aria-pressed=move || (channel.get().as_deref() == Some(name)).to_string()
                    on:click=move |_| channel.set(Some(name.into()))
                >{format!("/{name}")}</button>
            }).collect_view()}
        </nav>
        <div class="feed-heading">
            <h1>{move || channel.get().map_or_else(|| "Front page".into(), |c| format!("/{c}"))}</h1>
            <div class="sort-tabs" role="group" aria-label="Sort posts">
                <span>"sort:"</span>
                <button class:chosen=move || sort.get() == FeedSort::New aria-pressed=move || (sort.get() == FeedSort::New).to_string() on:click=move |_| sort.set(FeedSort::New)>"new"</button>
                <button class:chosen=move || sort.get() == FeedSort::Top aria-pressed=move || (sort.get() == FeedSort::Top).to_string() on:click=move |_| sort.set(FeedSort::Top)>"top"</button>
            </div>
            <span class="post-count" aria-live="polite">{move || format!("{} conversations", visible.get().len())}</span>
        </div>
        <ol class="post-list">
            <For each=move || visible.get() key=|post| post.id.clone() children=move |post| view! {
                <PostRow post/>
            }/>
        </ol>
        <Show when=move || visible.get().is_empty()>
            <div class="empty-state">
                <h2>"No conversations here yet."</h2>
                <p>"Try another source, channel, or search."</p>
                <button class="text-button" on:click=move |_| {
                    source.set(SourceFilter::All);
                    channel.set(None);
                    search.set(String::new());
                }>"clear filters"</button>
            </div>
        </Show>
        <div class="feed-end"><span>"you've reached the end of the sample feed"</span><span aria-hidden="true">"✳"</span></div>
    }
}

#[component]
fn PostRow(post: Post) -> AnyView {
    let expanded = RwSignal::new(false);
    let saved = RwSignal::new(false);
    let likes = post.display_metrics().map_or(0, |m| m.likes);
    let recasts = post.display_metrics().map_or(0, |m| m.recasts);
    let count = post.comment_count();
    let comment_label = if count == 1 {
        "1 comment".into()
    } else {
        format!("{count} comments")
    };
    let thread_id = format!("thread-{}", post.id);
    let thread_control = thread_id.clone();
    let thread_section = thread_id.clone();
    let metric_label = post
        .display_metrics()
        .map_or("No source snapshot".into(), |m| {
            format!(
                "{} likes from the {} sample snapshot",
                m.likes,
                m.network.label()
            )
        });
    let comments = post.comments;
    let body = post.body;

    view! {
        <li class="post-row" data-post-id=post.id>
            <div class="post-summary">
                <span class="rank" aria-hidden="true"></span>
                <div class="post-score" title=metric_label>
                    <span class="like-symbol" aria-hidden="true">"♡"</span>
                    <strong>{likes}</strong>
                    <span class="likes-label">"likes"</span>
                </div>
                <div class="post-main">
                    <div class="post-title-line">
                        <h2><button class="post-title" on:click=move |_| expanded.update(|value| *value = !*value) aria-expanded=move || expanded.get().to_string() aria-controls=thread_control.clone()>{post.title}</button></h2>
                        <div class="source-badges" aria-label="Observed in these sample sources">
                            {post.metrics.into_iter().map(|m| view! { <span class=format!("source-badge {}", m.network.class())>{m.network.label()}</span> }).collect_view()}
                        </div>
                    </div>
                    <p class="post-meta"><span class="author">{format!("@{}", post.author)}</span>" · "{post.age}" ago in "<span class="channel-name">{format!("/{}", post.channel)}</span></p>
                    <p class="post-excerpt">{body.clone()}</p>
                    <div class="post-actions">
                        <button class="thread-toggle" aria-expanded=move || expanded.get().to_string() aria-controls=thread_id.clone() on:click=move |_| expanded.update(|value| *value = !*value)>{comment_label}</button>
                        <span>{format!("{recasts} recasts")}</span>
                        <button class:saved=move || saved.get() aria-pressed=move || saved.get().to_string() title="Saved only in this preview until the page is reloaded" on:click=move |_| saved.update(|value| *value = !*value)>{move || if saved.get() { "saved for this visit" } else { "save" }}</button>
                        <span class="sample-label">"sample"</span>
                    </div>
                </div>
            </div>
            <section class="thread" id=thread_section hidden=move || !expanded.get() aria-label="Sample discussion">
                <p class="thread-body">{body}</p>
                <div class="thread-divider"><h3>"The conversation"</h3><span>"fictional replies"</span></div>
                {if comments.is_empty() {
                    view! { <p class="no-comments">"A little quiet here. This sample has no replies yet."</p> }.into_any()
                } else {
                    view! { <div class="comments">{comments.into_iter().map(|comment| view! { <CommentNode comment/> }).collect_view()}</div> }.into_any()
                }}
                <p class="reply-note">"Reading works in this seed. Writing and account connections are coming later."</p>
            </section>
        </li>
    }.into_any()
}

#[component]
fn CommentNode(comment: Comment) -> AnyView {
    let collapsed = RwSignal::new(false);
    let count = comment.total_count() - 1;
    let body_id = format!("comment-{}", comment.id);
    let controls = body_id.clone();
    let replies = comment.replies;
    view! {
        <article class="comment">
            <div class="comment-meta">
                <button class="collapse-comment" on:click=move |_| collapsed.update(|value| *value = !*value) aria-expanded=move || (!collapsed.get()).to_string() aria-controls=controls aria-label="Toggle comment and replies">{move || if collapsed.get() { "[+]" } else { "[−]" }}</button>
                <span class="author">{format!("@{}", comment.author)}</span>
                <span>{format!("{} ago", comment.age)}</span>
                <Show when=move || collapsed.get()><span>{format!("{count} replies hidden")}</span></Show>
            </div>
            <div id=body_id hidden=move || collapsed.get()>
                <p>{comment.body}</p>
                <div class="comment-replies">{replies.into_iter().map(|comment| view! { <CommentNode comment/> }.into_any()).collect_view()}</div>
            </div>
        </article>
    }.into_any()
}

#[component]
fn Sidebar() -> impl IntoView {
    view! {
        <aside class="sidebar" aria-label="About this preview">
            <section class="sidebar-card welcome-card">
                <div class="card-eyebrow">"HELLO, WORLD"</div>
                <h2>"More room for words."</h2>
                <p>"An independent home for Farcaster conversations. Familiar threads, a quieter interface, and a choice of sources."</p>
                <A href="/about" attr:class="sidebar-link">"Meet Allthing →"</A>
                <div class="seed-stamp"><span class="status-dot"></span>"v0.1 · the beginning"</div>
            </section>
            <section class="sidebar-card">
                <h2 class="small-heading">"Two sources. One reading space."</h2>
                <p>"Snapchain and Hypersnap can carry the same conversations. Source labels show which sample snapshots are available."</p>
                <p class="sidebar-note">"Counts from overlapping sources are never added together."</p>
            </section>
            <section class="sidebar-card preview-checklist">
                <h2 class="small-heading">"In this seed"</h2>
                <ul>
                    <li><span>"✓"</span>" Server-rendered pages"</li>
                    <li><span>"✓"</span>" Rust + WebAssembly interactions"</li>
                    <li><span>"✓"</span>" Filters & nested discussions"</li>
                    <li class="planned"><span>"○"</span>" Live feeds & QR sign-in"</li>
                </ul>
                <a href="https://github.com/ael-dev3/Allthing" rel="noreferrer">"follow the source →"</a>
            </section>
            <p class="sidebar-fineprint">"All handles, posts, and counts shown here are fictional. Save actions last until this page is reloaded or the post leaves the current view."</p>
        </aside>
    }
}

#[component]
fn About() -> impl IntoView {
    view! {
        <main id="main" class="about-page">
            <div class="card-eyebrow">"THE IDEA"</div>
            <h1>"A place for the conversation."</h1>
            <p class="about-lead">"Allthing is an independent Farcaster web client taking shape in Rust. It brings the compact lists and nested discussions of the early web to a modern, open social network."</p>
            <section><h2>"Read across sources"</h2><p>"The planned client will read from Snapchain and Hypersnap through separate adapters. A shared post should appear once, with its origins visible. One provider's moderation decision is not automatically another provider's view."</p></section>
            <section><h2>"Connect deliberately"</h2><p>"Farcaster QR sign-in is planned, followed by a compatible independent authentication path. Login and permission to publish are separate steps. This seed does not request account access, create signers, or publish posts."</p></section>
            <section><h2>"Keep the interface simple"</h2><p>"Light mode, readable threads, chronological feeds, source filters, and clear user controls. Likes and recasts keep their protocol meaning; there are no pretend network downvotes."</p></section>
            <section><h2>"What works today"</h2><p>"A native Rust server renders the page and serves fictional feed data. The same Leptos components hydrate in your browser as WebAssembly. Try source and channel filters, search, sorting, opening threads, collapsing replies, and temporary saves."</p></section>
            <section><h2>"Build in the open"</h2><p>"Allthing's original code is MIT licensed. Live adapters, authentication, posting, storage, and production hardening are future work. This project is independent and is not an official Farcaster client."</p></section>
            <div class="about-links"><A href="/">"← explore the seed"</A><a href="https://github.com/ael-dev3/Allthing" rel="noreferrer">"source & roadmap →"</a></div>
        </main>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    #[cfg(feature = "ssr")]
    if let Some(response) = use_context::<leptos_axum::ResponseOptions>() {
        response.set_status(http::StatusCode::NOT_FOUND);
    }
    view! {
        <main id="main" class="about-page">
            <h1>"This corner is empty."</h1>
            <p>"We couldn't find that page."</p>
            <A href="/">"Back to the front page →"</A>
        </main>
    }
}

#[component]
fn Footer() -> impl IntoView {
    view! {
        <footer class="site-footer">
            <span><strong>"allthing."</strong>" an independent client, taking shape."</span>
            <span><A href="/about">"about"</A>" · "<a href="https://github.com/ael-dev3/Allthing" rel="noreferrer">"source"</a>" · MIT licensed"</span>
        </footer>
    }
}
