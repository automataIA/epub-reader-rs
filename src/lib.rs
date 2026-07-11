use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::{components::*, path};

pub mod epub;
pub mod models;
pub mod reader;
pub mod storage;
pub mod tts;
pub mod ui;

use crate::ui::ReaderApp;

/// An app router which renders the homepage and handles 404's
#[component]
pub fn App() -> impl IntoView {
    // Provides context that manages stylesheets, titles, meta tags, etc.
    provide_meta_context();

    view! {
        <Html attr:lang="en" attr:dir="ltr" attr:data-theme="lampada" />

        // sets the document title
        <Title text="EPUB Reader RS" />

        // injects metadata in the <head> of the page
        <Meta charset="UTF-8" />
        <Meta name="viewport" content="width=device-width, initial-scale=1.0" />

        <Router base=if cfg!(feature = "gh-pages") { "/epub-reader-rs" } else { "" }>
            <Routes fallback=|| view! { <ReaderApp /> }>
                <Route path=path!("/") view=ReaderApp />
            </Routes>
        </Router>
    }
}
