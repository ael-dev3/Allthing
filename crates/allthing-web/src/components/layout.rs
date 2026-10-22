// SPDX-License-Identifier: AGPL-3.0-or-later
//! Page frame: header with the wordmark, footer, sidebar boxes, sort tabs,
//! pagination and loading or error states.

use allthing_core::ranking::{Sort, TimeRange};
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_location;

use crate::api::Failure;
use crate::components::search::SearchBox;
use crate::util::{BASE, GITHUB_MARK, SOURCE_URL, href, use_go, with_query};

#[component]
pub fn Wordmark() -> impl IntoView {
    view! { <a class="wordmark" href=href("/") aria-label="Allthing, home">"Allthing"</a> }
}

#[component]
pub fn SiteHeader() -> impl IntoView {
    let location = use_location();
    let current = move |prefix: &'static str| {
        let path = location.pathname.get();
        let path = path.strip_prefix(BASE).unwrap_or(&path);
        if prefix == "/" { path == "/" || path.is_empty() } else { path.starts_with(prefix) }
    };
    view! {
        <header class="site-header">
            <div class="header-inner">
                <Wordmark/>
                <nav class="main-nav" aria-label="Main">
                    <a href=href("/") aria-current=move || current("/").then_some("page")>"Home"</a>
                </nav>
                <SearchBox/>
                {account()}
            </div>
            <div class="portal-line" aria-hidden="true"></div>
        </header>
    }
}

/// Cast, on the pages that have it, and sign-in, or who is signed in with
/// their notifications' bell: in front of the Worker only. The static copy
/// has no sign-in, so no Cast.
#[cfg(not(feature = "static-host"))]
fn account() -> AnyView {
    view! {
        <div class="header-actions">
            <crate::components::compose::HeaderCast/>
            <crate::components::notifications::Bell/>
            <crate::components::signin::Account/>
        </div>
    }
    .into_any()
}

/// The static copy has no Worker to sign in with: it says it only reads.
#[cfg(feature = "static-host")]
fn account() -> AnyView {
    view! {
        <span class="readonly-pill" aria-description="This copy of Allthing only reads public Farcaster data. It has no sign-in and never posts.">
            "read-only"
        </span>
    }
    .into_any()
}

#[component]
pub fn SiteFooter() -> impl IntoView {
    view! {
        <footer class="site-footer">
            <a href=format!("{SOURCE_URL}/blob/main/LICENSE")>"AGPL-3.0"</a>
            <span aria-hidden="true">" · "</span>
            <a class="footer-source" href=SOURCE_URL target="_blank" rel="noopener" aria-label="Source on GitHub">
                <svg class="github-mark" viewBox="0 0 16 16" aria-hidden="true"><path d=GITHUB_MARK/></svg>
                "Source"
            </a>
            <span aria-hidden="true">" · "</span>
            <span class="version">{format!("v{} ({})", crate::VERSION, crate::COMMIT)}
            </span>
        </footer>
    }
}

#[component]
pub fn SideBox(#[prop(into)] title: String, children: Children) -> impl IntoView {
    view! {
        <section class="side-box">
            <h2>{title}</h2>
            {children()}
        </section>
    }
}

/// Main column plus sidebar. `class` adds a modifier, such as
/// `profile-columns`, which moves the sidebar under the profile header on
/// narrow screens.
#[component]
pub fn Columns(
    /// Sidebar content; without it the page is one centered column.
    #[prop(optional)]
    side: Option<AnyView>,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let single = if side.is_none() { " single" } else { "" };
    view! {
        <div class=format!("columns{single} {class}")>
            <div class="content">{children()}</div>
            {side.map(|side| view! { <aside class="side" aria-label="Sidebar">{side}</aside> })}
        </div>
    }
}

/// hot · new · top links for a listing at `path`.
#[component]
pub fn SortTabs(path: String, current: Sort, #[prop(optional)] default_sort: Option<Sort>) -> impl IntoView {
    let default_sort = default_sort.unwrap_or(Sort::Hot);
    view! {
        <nav class="sort-tabs" aria-label="Sort">
            {Sort::ALL
                .into_iter()
                .map(|sort| {
                    let href = with_query(&path, &[("sort", (sort != default_sort).then(|| sort.as_str().to_owned()))]);
                    view! {
                        <a href=href data-noscroll="" class:selected=sort == current aria-current=(sort == current).then_some("page") aria-description=sort.describe()>
                            {sort.as_str()}
                        </a>
                    }
                })
                .collect_view()}
        </nav>
    }
}

/// Whether the menu offers `range`. The static copy has no index, so it
/// leaves out the ranges ranked from one.
fn offered(range: TimeRange) -> bool {
    !(cfg!(feature = "static-host") && range.window().is_indexed())
}

/// Reddit's time filter: how far back the front page reaches, kept in the
/// URL as `t` next to the sort.
#[component]
pub fn TimeFilter(path: String, sort: Sort, current: TimeRange) -> impl IntoView {
    let navigate = use_go();
    let change = move |ev: leptos::ev::Event| {
        let range = TimeRange::parse(Some(&event_target_value(&ev)));
        let href = with_query(
            &path,
            &[
                ("sort", (sort != Sort::Hot).then(|| sort.as_str().to_owned())),
                ("t", (range != TimeRange::default()).then(|| range.as_str().to_owned())),
            ],
        );
        navigate(&href, NavigateOptions { scroll: false, ..Default::default() });
    };
    view! {
        <label class="time-filter">
            <span class="visually-hidden">"Time range"</span>
            <select on:change=change>
                {TimeRange::ALL
                    .into_iter()
                    .filter(|range| offered(*range) || *range == current)
                    .map(|range| view! { <option value=range.as_str() selected=range == current>{range.label()}</option> })
                    .collect_view()}
            </select>
        </label>
    }
}

/// Says so when a page shows a cached copy because the data source isn't
/// answering; nothing otherwise.
#[component]
pub fn StaleNotice(stale: bool) -> impl IntoView {
    stale.then(|| {
        view! {
            <p class="stale-notice" role="status">
                "Showing a cached copy: the Farcaster data source is not answering right now."
            </p>
        }
    })
}

/// What an error panel tells a reader: what happened, as a heading and a
/// line, the page's title, and whether trying again can help.
#[derive(Debug, PartialEq)]
struct Explained {
    heading: &'static str,
    line: &'static str,
    title: &'static str,
    retry: Retry,
}

/// Whether, and how, an error panel tries again.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Retry {
    /// It can't help: a link to nothing, or a search too long.
    Never,
    /// A button: the network or the Worker may answer next time.
    Button,
    /// The data source is busy: on its own after this many seconds, with a
    /// button meanwhile.
    After(u32),
}

/// A busy page tries again on its own this many times in a row, then
/// leaves it to the reader.
const AUTO_TRIES: u32 = 3;

/// The words for `failure`: a reader's, not the API's. Its messages ("No
/// such cast", "Farcaster data source is busy; backing off") pick them,
/// and aren't shown.
fn explain(failure: &Failure) -> Explained {
    use allthing_core::api::{NO_SUCH_CAST, NO_SUCH_CHANNEL, NO_SUCH_USER, SEARCH_TOO_LONG};
    let says = |heading, line, title, retry| Explained { heading, line, title, retry };
    match failure {
        Failure::NotFound(message) => match message.as_str() {
            NO_SUCH_CAST => {
                says("No such cast", "It doesn't exist, or it was deleted.", "Cast not found", Retry::Never)
            }
            NO_SUCH_USER => {
                says("No such account", "No Farcaster account goes by that name.", "Account not found", Retry::Never)
            }
            NO_SUCH_CHANNEL => {
                says("No such channel", "No Farcaster channel goes by that name.", "Channel not found", Retry::Never)
            }
            _ => says("Nothing here", "There's nothing at this address.", "Not found", Retry::Never),
        },
        Failure::Refused(message) if message == SEARCH_TOO_LONG => {
            says("That search is too long", "Try it with fewer words.", "Search too long", Retry::Never)
        }
        Failure::Refused(_) => says(
            "This link doesn't work",
            "Allthing can't open this address. Check the link, or start from the front page.",
            "Can't open this link",
            Retry::Never,
        ),
        Failure::Busy { retry_after, .. } => says(
            "Farcaster's data is busy",
            "Many people are reading at once.",
            "Busy",
            Retry::After(retry_after.unwrap_or(5).clamp(2, 60)),
        ),
        Failure::Failed(_) if failure.unreachable() => {
            says("Can't reach Allthing", "Check your connection, then try again.", "Offline", Retry::Button)
        }
        Failure::Failed(_) => says(
            "Something went wrong",
            "Allthing couldn't load this just now. Try again in a moment.",
            "Couldn't load",
            Retry::Button,
        ),
    }
}

/// Whether a busy page may try again on its own: not after `AUTO_TRIES`
/// tries of the same page in a row, each within five minutes of the last
/// (counted in this tab's session storage; without it, never).
fn may_try_again() -> bool {
    const KEY: &str = "allthing-busy-tries";
    let Some(storage) = window().session_storage().ok().flatten() else { return false };
    let here = window().location().href().unwrap_or_default();
    let now = js_sys::Date::now();
    let kept = storage.get_item(KEY).ok().flatten().unwrap_or_default();
    let mut kept = kept.splitn(3, ' ');
    let tries = match (
        kept.next().and_then(|n| n.parse::<u32>().ok()),
        kept.next().and_then(|at| at.parse::<f64>().ok()),
        kept.next(),
    ) {
        (Some(tries), Some(at), Some(page)) if page == here && now - at < 300_000.0 => tries,
        _ => 0,
    };
    tries < AUTO_TRIES && storage.set_item(KEY, &format!("{} {now} {here}", tries + 1)).is_ok()
}

/// When a busy page tries again on its own, `seconds` from now, in words.
fn in_a_while(seconds: u32) -> String {
    match seconds {
        0..=9 => "in a few seconds".to_owned(),
        60.. => "in a minute".to_owned(),
        _ => format!("in {seconds} seconds"),
    }
}

/// A page that couldn't load: what happened, in a reader's words, and
/// what they can do. A busy data source is tried again on its own; a link
/// to nothing, or a search too long, has no "Try again", which couldn't
/// help. The page's title says it too.
#[component]
pub fn ErrorPanel(failure: Failure) -> impl IntoView {
    let explained = explain(&failure);
    crate::util::set_title(explained.title);
    let reload = || {
        let _ = window().location().reload();
    };
    let auto = match explained.retry {
        Retry::After(seconds) if may_try_again() => Some(seconds),
        _ => None,
    };
    if let Some(seconds) = auto
        && let Ok(timer) = set_timeout_with_handle(reload, std::time::Duration::from_secs(seconds.into()))
    {
        on_cleanup(move || timer.clear());
    }
    let then = match (explained.retry, auto) {
        (Retry::After(_), Some(seconds)) => Some(format!("Allthing tries again {}.", in_a_while(seconds))),
        (Retry::After(_), None) => Some("Try again in a minute.".to_owned()),
        _ => None,
    };
    let button = match explained.retry {
        Retry::Never => None,
        Retry::Button => Some("Try again"),
        Retry::After(_) => Some(if auto.is_some() { "Try again now" } else { "Try again" }),
    };
    view! {
        <section class="error-panel" role="alert">
            <h1>{explained.heading}</h1>
            <p>{explained.line}{then.map(|then| format!(" {then}"))}</p>
            {button.map(|label| view! { <button type="button" class="button" on:click=move |_| reload()>{label}</button> })}
            <p><a href=href("/")>"Back to the front page"</a></p>
        </section>
    }
}

#[component]
pub fn NotFound() -> impl IntoView {
    crate::util::set_title("Not found");
    view! {
        <section class="error-panel not-found">
            <h1>"Page not found"</h1>
            <p>"This corner of the Allthing is empty."</p>
            <p><a href=href("/")>"Back to the front page"</a></p>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_says_what_happened_and_what_a_reader_can_do() {
        let not_found = |message: &str| explain(&Failure::NotFound(message.into()));
        assert_eq!(
            (not_found("No such cast").heading, not_found("No such cast").title),
            ("No such cast", "Cast not found")
        );
        assert_eq!(not_found("No such user").title, "Account not found");
        assert_eq!(not_found("No such channel").title, "Channel not found");
        assert_eq!(not_found("Unknown endpoint").heading, "Nothing here");
        for missing in ["No such cast", "No such user", "No such channel", "Unknown endpoint"] {
            assert_eq!(not_found(missing).retry, Retry::Never);
        }
        // A search too long: said plainly, and no "Try again".
        let long = explain(&Failure::Refused("Search query too long".into()));
        assert_eq!(
            (long.heading, long.title, long.retry),
            ("That search is too long", "Search too long", Retry::Never)
        );
        assert_eq!(explain(&Failure::Refused("Expected ?hash=<40 hex characters>".into())).retry, Retry::Never);
        // Busy: tried again on its own, after what the Worker says, within reason.
        let busy = |retry_after| {
            explain(&Failure::Busy { message: "Farcaster data source is busy; backing off".into(), retry_after })
        };
        assert_eq!(busy(Some(5)).retry, Retry::After(5));
        assert_eq!(busy(Some(0)).retry, Retry::After(2));
        assert_eq!(busy(Some(600)).retry, Retry::After(60));
        assert_eq!(busy(None).retry, Retry::After(5));
        // And when, as the Worker says.
        assert_eq!(in_a_while(5), "in a few seconds");
        assert_eq!(in_a_while(30), "in 30 seconds");
        assert_eq!(in_a_while(60), "in a minute");
        // Offline, or the Worker failing: a button.
        let offline = explain(&Failure::Failed(crate::api::UNREACHABLE.into()));
        assert_eq!((offline.heading, offline.retry), ("Can't reach Allthing", Retry::Button));
        let failed = explain(&Failure::Failed("upstream timed out".into()));
        assert_eq!((failed.heading, failed.retry), ("Something went wrong", Retry::Button));
        // None shows the API's own words.
        for said in [failed, offline, busy(None), long] {
            for words in [said.heading, said.line] {
                assert!(
                    !words.contains("upstream") && !words.contains("backing off") && !words.contains("query"),
                    "{words}"
                );
            }
        }
    }
}
