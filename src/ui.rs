use crate::epub::{EpubBook, cover_data_uri};
use crate::models::{
    Annotation, AnnotationKind, BookRecord, HighlightColor, ReaderFlow, ReaderFont,
    ReaderLocation, ReaderPreferences, ReaderTheme, SearchResult, StoredBook, TextQuoteSelector,
    TocItem,
};
use crate::reader::{reader_css, theme_colors, tts_text_from_html};
use crate::storage::AppStorage;
use crate::tts::{self, PiperState};
use gloo_timers::future::TimeoutFuture;
use icondata as icons;
use leptos::ev::{DragEvent, Event};
use leptos::html::Iframe;
use leptos::prelude::*;
use leptos_icons::Icon;
use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{File, HtmlInputElement};

thread_local! {
    static STORAGE: RefCell<Option<Rc<AppStorage>>> = const { RefCell::new(None) };
}

fn set_global_storage(storage: Rc<AppStorage>) {
    STORAGE.with(|slot| {
        *slot.borrow_mut() = Some(storage);
    });
}

fn get_storage() -> Option<Rc<AppStorage>> {
    STORAGE.with(|slot| slot.borrow().clone())
}

/// Stato del player di lettura ad alta voce mostrato in UI.
#[derive(Clone, Debug, PartialEq)]
enum TtsUiState {
    Idle,
    /// Download del modello voce: frazione 0..=1.
    Downloading(f64),
    /// In lettura: (frase corrente, frasi totali del capitolo).
    Speaking(u32, u32),
    Paused,
    Error(String),
}

impl TtsUiState {
    /// La voce sta leggendo (o scaricando il modello per farlo)?
    fn is_active(&self) -> bool {
        matches!(self, TtsUiState::Downloading(_) | TtsUiState::Speaking(..))
    }
}

#[component]
pub fn ReaderApp() -> impl IntoView {
    let (books, set_books) = signal(Vec::<StoredBook>::new());
    let (active_book, set_active_book) = signal(None::<EpubBook>);
    let (selected_id, set_selected_id) = signal(None::<String>);
    let (spine_index, set_spine_index) = signal(0usize);
    let (preferences, set_preferences) = signal(ReaderPreferences::default());
    let (annotations, set_annotations) = signal(Vec::<Annotation>::new());
    let (query, set_query) = signal(String::new());
    let (library_filter, set_library_filter) = signal(String::new());
    let (sort_mode, set_sort_mode) = signal("recent".to_string());
    let (search_results, set_search_results) = signal(Vec::<SearchResult>::new());
    let (status, set_status) = signal("Ready to import an EPUB.".to_string());
    let (note_draft, set_note_draft) = signal(String::new());
    let (highlight_draft, set_highlight_draft) = signal(String::new());
    let (active_tab, set_active_tab) = signal("toc".to_string());
    let (zen, set_zen) = signal(false);
    let (show_shortcuts, set_show_shortcuts) = signal(false);
    let (ctx_menu, set_ctx_menu) = signal(None::<(f64, f64, String)>);
    let (tts_state, set_tts_state) = signal(TtsUiState::Idle);
    // Richiesta di stop: fa uscire il loop di lettura invece di avanzare capitolo.
    let (tts_stop, set_tts_stop) = signal(false);
    let iframe_ref = NodeRef::<Iframe>::new();
    let shortcuts_dialog = NodeRef::<leptos::html::Dialog>::new();

    // Drive the native <dialog> from the signal: showModal() gives free focus
    // trapping, background inert-ing and focus restore on close.
    Effect::new(move |_| {
        let Some(dialog) = shortcuts_dialog.get() else {
            return;
        };
        if show_shortcuts.get() {
            let _ = dialog.show_modal();
        } else {
            dialog.close();
        }
    });

    spawn_local(async move {
        match AppStorage::open().await {
            Ok(repo) => {
                let repo = Rc::new(repo);
                // Registrato subito: gli Effect che partono al mount (es. il
                // check del modello TTS in cache) devono trovare lo storage.
                set_global_storage(repo.clone());
                let prefs = repo.get_preferences().await;
                set_preferences.set(prefs);
                match repo.list_books().await {
                    Ok(mut loaded) => {
                        sort_books(&mut loaded, "recent");
                        set_status.set(format!("{} local books loaded.", loaded.len()));
                        set_books.set(loaded);
                    }
                    Err(err) => {
                        set_status.set(format!("Storage opened, but the library could not be read: {err}"))
                    }
                }
            }
            Err(err) => {
                set_status.set(format!(
                    "IndexedDB unavailable: this session stays in memory. Details: {err}"
                ));
            }
        }
    });

    let selected_book = move || {
        let selected = selected_id.get();
        books
            .get()
            .into_iter()
            .find(|book| Some(book.record.id.clone()) == selected)
    };

    let chapter_html = move || {
        let Some(book) = active_book.get() else {
            return empty_reader_doc(&preferences.get());
        };
        let css = reader_css(&preferences.get());
        book.render_spine_item(spine_index.get(), &css)
            .unwrap_or_else(|err| error_reader_doc(&format!("{err}"), &css))
    };

    let open_book = move |book: StoredBook| {
        let id = book.record.id.clone();
        let storage_value = get_storage();
        set_status.set(format!("Opening {}.", book.record.title));
        match EpubBook::from_bytes(&book.bytes) {
            Ok(epub) => {
                set_active_book.set(Some(epub));
                set_selected_id.set(Some(id.clone()));
                set_spine_index.set(0);
                set_search_results.set(Vec::new());
                if let Some(repo) = storage_value.clone() {
                    let id_for_location = id.clone();
                    spawn_local(async move {
                        if let Ok(Some(location)) = repo.get_location(&id_for_location).await {
                            set_spine_index.set(location.spine_index);
                        }
                    });
                }
                if let Some(repo) = storage_value {
                    spawn_local(async move {
                        match repo.list_annotations(&id).await {
                            Ok(items) => set_annotations.set(items),
                            Err(err) => set_status.set(format!("Could not load notes: {err}")),
                        }
                    });
                } else {
                    set_annotations.set(Vec::new());
                }
            }
            Err(err) => set_status.set(format!("EPUB import failed: {err}")),
        }
    };

    let save_current_location = move || {
        let Some(book_id) = selected_id.get() else {
            return;
        };
        let location = ReaderLocation {
            book_id,
            spine_index: spine_index.get(),
            scroll_ratio: 0.0,
            element_id: None,
        };
        if let Some(repo) = get_storage() {
            spawn_local(async move {
                let _ = repo.save_location(&location).await;
            });
        }
    };

    let persist_preferences = move |next: ReaderPreferences| {
        set_preferences.set(next.clone());
        if let Some(repo) = get_storage() {
            spawn_local(async move {
                let _ = repo.save_preferences(&next).await;
            });
        }
    };

    let stop_listening = move || {
        set_tts_stop.set(true);
        tts::stop();
        set_tts_state.set(TtsUiState::Idle);
    };

    // Avvia (o riprende) la lettura dal capitolo corrente e prosegue coi
    // successivi. Il modello Piper si scarica alla prima riproduzione (poi resta
    // in cache OPFS del browser); l'inferenza gira in JS, fuori dal thread UI.
    let start_listening = move || {
        match tts_state.get_untracked() {
            TtsUiState::Speaking(..) | TtsUiState::Downloading(_) => return,
            TtsUiState::Paused => {
                tts::resume();
                set_tts_state.set(TtsUiState::Speaking(0, 0));
                return;
            }
            _ => {}
        }
        if active_book.get_untracked().is_none() {
            set_status.set("Open a book before starting the reading voice.".into());
            return;
        }
        set_tts_stop.set(false);
        spawn_local(async move {
            let prefs = preferences.get_untracked();
            let voice = tts::voice_for_language(&prefs.tts.language).voice_id;

            // Un capitolo alla volta, avanzando finché c'è testo e nessuno Stop.
            // Il primo capitolo parte dalla frase *visibile* a schermo; i successivi
            // (auto-avanzati) dall'inizio.
            let mut first = true;
            while let Some(book) = active_book.get_untracked() {
                if tts_stop.get_untracked() {
                    break;
                }
                let index = spine_index.get_untracked();
                let Ok(html) = book.spine_item_html(index) else {
                    break;
                };
                let full =
                    tts_text_from_html(&html, &prefs.tts.language, prefs.tts.read_code_blocks);
                let text = if first {
                    visible_chapter_text(iframe_ref).unwrap_or(full)
                } else {
                    full
                };
                first = false;
                if !text.trim().is_empty() {
                    tts::start(voice, &text, prefs.tts.speed);
                    // Polla lo stato del glue JS finché il capitolo non finisce.
                    loop {
                        if tts_stop.get_untracked() {
                            tts::stop();
                            set_tts_state.set(TtsUiState::Idle);
                            return;
                        }
                        match tts::read_status() {
                            PiperState::Downloading(frac) => {
                                set_tts_state.set(TtsUiState::Downloading(frac))
                            }
                            PiperState::Speaking { sentence, total } => {
                                set_tts_state.set(TtsUiState::Speaking(sentence, total))
                            }
                            PiperState::Paused => set_tts_state.set(TtsUiState::Paused),
                            PiperState::Error(err) => {
                                set_tts_state.set(TtsUiState::Error(err));
                                return;
                            }
                            PiperState::Idle => break, // capitolo finito
                        }
                        TimeoutFuture::new(150).await;
                    }
                }
                let last = book.package.spine.len().saturating_sub(1);
                if index >= last {
                    break;
                }
                set_spine_index.set(index + 1);
            }
            set_tts_state.set(TtsUiState::Idle);
        });
    };

    let pause_listening = move || {
        if matches!(tts_state.get_untracked(), TtsUiState::Speaking(..)) {
            tts::pause();
            set_tts_state.set(TtsUiState::Paused);
        }
    };

    let import_file = move |file: File| {
        let repo = get_storage();
        spawn_local(async move {
            set_status.set(format!("Importing {}...", file.name()));
            match read_file_bytes(file).await.and_then(import_book) {
                Ok(stored) => {
                    if let Some(repo) = repo
                        && let Err(err) = repo.save_book(&stored).await
                    {
                        set_status.set(format!("Book imported but not saved: {err}"));
                    }
                    let mut next_books = books.get_untracked();
                    next_books.retain(|book| book.record.id != stored.record.id);
                    next_books.push(stored.clone());
                    sort_books(&mut next_books, &sort_mode.get_untracked());
                    set_books.set(next_books);
                    set_status.set(format!("{} imported.", stored.record.title));
                    open_book(stored);
                }
                Err(err) => set_status.set(err),
            }
        });
    };

    let run_search = move || {
        let query_value = query.get();
        if let Some(book) = active_book.get() {
            set_search_results.set(book.search(&query_value));
        }
    };

    let go_prev = move || {
        let index = spine_index.get_untracked();
        if index > 0 {
            set_spine_index.set(index - 1);
            save_current_location();
        }
    };

    let go_next = move || {
        if let Some(book) = active_book.get_untracked() {
            let index = spine_index.get_untracked();
            if index + 1 < book.package.spine.len() {
                set_spine_index.set(index + 1);
                save_current_location();
            }
        }
    };

    // In paged flow, arrows first move by one column page inside the chapter,
    // then cross chapter boundaries; in scroll flow they change chapter.
    let page_forward = move || {
        if preferences.get_untracked().flow == ReaderFlow::Paged
            && let Some(body) = iframe_body(iframe_ref)
        {
            let step = body.client_width();
            let left = body.scroll_left();
            let max = body.scroll_width() - step;
            if left < max - 8 {
                body.set_scroll_left(left + step);
                return;
            }
        }
        go_next();
    };

    let page_backward = move || {
        if preferences.get_untracked().flow == ReaderFlow::Paged
            && let Some(body) = iframe_body(iframe_ref)
        {
            let left = body.scroll_left();
            if left > 8 {
                body.set_scroll_left(left - body.client_width());
                return;
            }
        }
        go_prev();
    };

    // La modalità zen usa la Fullscreen API: schermo intero vero, non solo
    // il layout. fullscreenchange tiene lo stato allineato quando il browser
    // esce da solo (es. Esc nativo).
    let set_zen_mode = move |enable: bool| {
        set_zen.set(enable);
        set_browser_fullscreen(enable);
    };
    let toggle_zen = move || set_zen_mode(!zen.get_untracked());

    {
        let fs_closure = Closure::<dyn FnMut()>::new(move || {
            if document().fullscreen_element().is_none() {
                set_zen.set(false);
            }
        });
        let _ = document()
            .add_event_listener_with_callback("fullscreenchange", fs_closure.as_ref().unchecked_ref());
        fs_closure.forget();
    }

    let open_note_editor = move || {
        if let Some(selection) = selection_text_from_iframe(iframe_ref) {
            let trimmed = selection.trim().to_string();
            if !trimmed.is_empty() {
                set_highlight_draft.set(trimmed);
            }
        }
        set_zen_mode(false);
        set_active_tab.set("notes".into());
        set_ctx_menu.set(None);
    };

    let add_bookmark = move || {
        let Some(book_id) = selected_id.get_untracked() else {
            set_status.set("Open a book before adding a bookmark.".into());
            return;
        };
        let Some(book) = active_book.get_untracked() else {
            return;
        };
        let Some(spine) = book.package.spine.get(spine_index.get_untracked()) else {
            return;
        };
        let exact = selection_text_from_iframe(iframe_ref)
            .map(|text| text.trim().to_string())
            .unwrap_or_default();
        let now = js_sys::Date::now();
        let annotation = Annotation {
            id: Uuid::new_v4().to_string(),
            book_id,
            spine_href: spine.href.clone(),
            selector: TextQuoteSelector {
                exact,
                prefix: None,
                suffix: None,
            },
            color: HighlightColor::Yellow,
            note: String::new(),
            kind: AnnotationKind::Bookmark,
            created_at: now,
            updated_at: now,
        };
        let mut next = annotations.get_untracked();
        next.push(annotation.clone());
        set_annotations.set(next);
        set_status.set("Bookmark added.".into());
        set_ctx_menu.set(None);
        if let Some(repo) = get_storage() {
            spawn_local(async move {
                let _ = repo.save_annotation(&annotation).await;
            });
        }
    };

    let add_annotation = move || {
        let Some(book_id) = selected_id.get() else {
            set_status.set("Open a book before saving a note.".into());
            return;
        };
        let Some(book) = active_book.get() else {
            return;
        };
        let Some(spine) = book.package.spine.get(spine_index.get()) else {
            return;
        };
        let exact = selection_text_from_iframe(iframe_ref)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| highlight_draft.get());
        if exact.trim().is_empty() && note_draft.get().trim().is_empty() {
            set_status.set("Select text in the chapter or write a note.".into());
            return;
        }
        let now = js_sys::Date::now();
        let annotation = Annotation {
            id: Uuid::new_v4().to_string(),
            book_id,
            spine_href: spine.href.clone(),
            selector: TextQuoteSelector {
                exact: exact.trim().to_string(),
                prefix: None,
                suffix: None,
            },
            color: HighlightColor::Yellow,
            note: note_draft.get().trim().to_string(),
            kind: AnnotationKind::Note,
            created_at: now,
            updated_at: now,
        };
        let mut next = annotations.get();
        next.push(annotation.clone());
        set_annotations.set(next);
        set_note_draft.set(String::new());
        set_highlight_draft.set(String::new());
        set_status.set("Note saved locally.".into());
        if let Some(repo) = get_storage() {
            spawn_local(async move {
                let _ = repo.save_annotation(&annotation).await;
            });
        }
    };

    let export_annotations = move || {
        let json = serde_json::to_string_pretty(&annotations.get()).unwrap_or_else(|_| "[]".into());
        set_status.set(format!(
            "Annotations export ready: {} JSON characters.",
            json.len()
        ));
        copy_to_clipboard(&json);
    };

    let handle_key = move |ev: &web_sys::KeyboardEvent, from_iframe: bool| {
        if ev.ctrl_key() || ev.meta_key() || ev.alt_key() {
            return;
        }
        if !from_iframe
            && let Some(el) = document().active_element()
            && matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
        {
            return;
        }
        match ev.key().as_str() {
            "ArrowRight" => {
                ev.prevent_default();
                page_forward();
            }
            "ArrowLeft" => {
                ev.prevent_default();
                page_backward();
            }
            // Inside a scrolled chapter, PageUp/PageDown keep their native
            // vertical scrolling; elsewhere they page through the book.
            "PageDown"
                if !(from_iframe && preferences.get_untracked().flow == ReaderFlow::Scroll) => {
                ev.prevent_default();
                page_forward();
            }
            "PageUp"
                if !(from_iframe && preferences.get_untracked().flow == ReaderFlow::Scroll) => {
                ev.prevent_default();
                page_backward();
            }
            // Single-character shortcuts (WCAG 2.1.4): active only while the
            // reading iframe is focused, so they never hijack global typing.
            // Elsewhere the toolbar buttons and context menu cover the same
            // actions.
            "n" | "N" if from_iframe => open_note_editor(),
            "b" | "B" if from_iframe => add_bookmark(),
            "f" | "F" if from_iframe => toggle_zen(),
            "?" if from_iframe => set_show_shortcuts.set(true),
            "Escape" => {
                if show_shortcuts.get_untracked() {
                    set_show_shortcuts.set(false);
                } else if ctx_menu.get_untracked().is_some() {
                    set_ctx_menu.set(None);
                } else {
                    set_zen_mode(false);
                }
            }
            _ => {}
        }
    };

    let _key_listener = window_event_listener(leptos::ev::keydown, move |ev| {
        handle_key(&ev, false);
    });

    // The chapter iframe is re-created on every render: re-attach listeners
    // for shortcuts and the selection context menu each time it loads.
    let on_frame_load = move |_| {
        let Some(iframe) = iframe_ref.get_untracked() else {
            return;
        };
        let Some(doc) = iframe.content_document() else {
            return;
        };

        let key_closure = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |ev: web_sys::KeyboardEvent| handle_key(&ev, true),
        );
        let _ = doc
            .add_event_listener_with_callback("keydown", key_closure.as_ref().unchecked_ref());
        key_closure.forget();

        let ctx_closure =
            Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |ev: web_sys::MouseEvent| {
                let selection = selection_text_from_iframe(iframe_ref)
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty());
                let Some(text) = selection else {
                    set_ctx_menu.set(None);
                    return;
                };
                ev.prevent_default();
                let (offset_x, offset_y) = iframe_ref
                    .get_untracked()
                    .map(|frame| {
                        let rect = frame.get_bounding_client_rect();
                        (rect.left(), rect.top())
                    })
                    .unwrap_or((0.0, 0.0));
                set_ctx_menu.set(Some((
                    f64::from(ev.client_x()) + offset_x,
                    f64::from(ev.client_y()) + offset_y,
                    text,
                )));
            });
        let _ = doc.add_event_listener_with_callback(
            "contextmenu",
            ctx_closure.as_ref().unchecked_ref(),
        );
        ctx_closure.forget();

        let click_closure = Closure::<dyn FnMut()>::new(move || set_ctx_menu.set(None));
        let _ = doc
            .add_event_listener_with_callback("mousedown", click_closure.as_ref().unchecked_ref());
        click_closure.forget();
    };

    view! {
        <main
            class="reader-shell bg-base-200 text-base-content"
            class:reader-zen=move || zen.get()
            on:mousedown=move |_| set_ctx_menu.set(None)
            on:dragover=move |ev: DragEvent| {
                ev.prevent_default();
            }
            on:drop=move |ev: DragEvent| {
                ev.prevent_default();
                if let Some(file) = ev
                    .data_transfer()
                    .and_then(|transfer| transfer.files())
                    .and_then(|files| files.get(0))
                {
                    import_file(file);
                }
            }
        >
            <aside class="reader-side reader-panel border-r overflow-hidden flex flex-col">
                <LibraryPanel
                    books=books
                    filter=library_filter
                    sort_mode=sort_mode
                    selected_id=selected_id
                    set_filter=set_library_filter
                    set_sort_mode=set_sort_mode
                    set_books=set_books
                    import_file=import_file
                    open_book=open_book
                />
            </aside>

            <section class="min-w-0 flex flex-col">
                <Toolbar
                    selected_book=Signal::derive(selected_book)
                    active_book=active_book
                    spine_index=spine_index
                    go_prev=go_prev
                    go_next=go_next
                    toggle_zen=toggle_zen
                    open_shortcuts=move || set_show_shortcuts.set(true)
                    tts_state=tts_state
                    toggle_listen=move || match tts_state.get_untracked() {
                        state if state.is_active() => pause_listening(),
                        _ => start_listening(),
                    }
                />

                <div
                    class="reader-page min-h-0 flex-1"
                    style=move || {
                        let (bg, ..) = theme_colors(preferences.get().theme);
                        format!("--reader-page-bg:{bg}")
                    }
                >
                    <iframe
                        node_ref=iframe_ref
                        class="reader-frame"
                        sandbox="allow-same-origin"
                        title="EPUB content"
                        srcdoc=chapter_html
                        on:load=on_frame_load
                    />
                </div>

                <div class="reader-status h-8 px-6 text-[11px] text-base-content/55 flex items-center justify-between gap-3">
                    <span class="truncate" role="status" aria-live="polite">{status}</span>
                    <span class="tabular-nums">{move || {
                        selected_book()
                            .map(|book| format!("{} chapters", book.package.spine.len()))
                            .unwrap_or_else(|| "No book open".into())
                    }}</span>
                </div>
            </section>

            <aside class="reader-side reader-panel border-l overflow-y-auto">
                <RightPanel
                    active_book=active_book
                    spine_index=spine_index
                    set_spine_index=set_spine_index
                    query=query
                    set_query=set_query
                    run_search=run_search
                    search_results=search_results
                    preferences=preferences
                    persist_preferences=persist_preferences
                    annotations=annotations
                    note_draft=note_draft
                    set_note_draft=set_note_draft
                    highlight_draft=highlight_draft
                    set_highlight_draft=set_highlight_draft
                    add_annotation=add_annotation
                    export_annotations=export_annotations
                    save_current_location=save_current_location
                    active_tab=active_tab
                    set_active_tab=set_active_tab
                    tts_state=tts_state
                    start_listen=start_listening
                    pause_listen=pause_listening
                    stop_listen=stop_listening
                />
            </aside>

            <Show when=move || zen.get()>
                <button
                    class="zen-exit"
                    aria-label="Exit full screen (F)"
                    title="Exit full screen (F)"
                    on:click=move |_| set_zen_mode(false)
                >
                    <Icon icon=icons::LuMinimize2 width="1rem" height="1rem" />
                </button>
            </Show>

            {move || {
                ctx_menu.get().map(|(x, y, selection)| {
                    let selection_for_note = selection.clone();
                    view! {
                        <div
                            class="ctx-menu"
                            style=format!("left:{x}px;top:{y}px")
                            on:mousedown=|ev| ev.stop_propagation()
                        >
                            <button
                                class="ctx-item"
                                on:click=move |_| {
                                    set_highlight_draft.set(selection_for_note.clone());
                                    set_zen_mode(false);
                                    set_active_tab.set("notes".into());
                                    set_ctx_menu.set(None);
                                }
                            >
                                <Icon icon=icons::LuHighlighter width=".9rem" height=".9rem" />
                                "Add note"
                            </button>
                            <button class="ctx-item" on:click=move |_| add_bookmark()>
                                <Icon icon=icons::LuBookmark width=".9rem" height=".9rem" />
                                "Add bookmark"
                            </button>
                        </div>
                    }
                })
            }}

            <dialog
                node_ref=shortcuts_dialog
                class="modal"
                aria-labelledby="shortcuts-title"
                on:close=move |_| set_show_shortcuts.set(false)
                on:click=move |ev| {
                    // A click landing on the <dialog> itself is the backdrop.
                    if ev
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::HtmlDialogElement>().ok())
                        .is_some()
                    {
                        set_show_shortcuts.set(false);
                    }
                }
            >
                    <div class="modal-box max-w-md">
                        <h3 id="shortcuts-title" class="book-title text-lg">"Keyboard shortcuts"</h3>
                        <table class="table table-sm mt-4">
                            <tbody>
                                <tr>
                                    <td><kbd class="kbd kbd-sm">"←"</kbd> " / " <kbd class="kbd kbd-sm">"Page Up"</kbd></td>
                                    <td>"Previous page / chapter"</td>
                                </tr>
                                <tr>
                                    <td><kbd class="kbd kbd-sm">"→"</kbd> " / " <kbd class="kbd kbd-sm">"Page Down"</kbd></td>
                                    <td>"Next page / chapter"</td>
                                </tr>
                                <tr>
                                    <td><kbd class="kbd kbd-sm">"N"</kbd></td>
                                    <td>"Add note from selection"</td>
                                </tr>
                                <tr>
                                    <td><kbd class="kbd kbd-sm">"B"</kbd></td>
                                    <td>"Add bookmark"</td>
                                </tr>
                                <tr>
                                    <td><kbd class="kbd kbd-sm">"F"</kbd></td>
                                    <td>"Toggle full screen reading"</td>
                                </tr>
                                <tr>
                                    <td><kbd class="kbd kbd-sm">"?"</kbd></td>
                                    <td>"Show this panel"</td>
                                </tr>
                                <tr>
                                    <td><kbd class="kbd kbd-sm">"Esc"</kbd></td>
                                    <td>"Close menus / exit full screen"</td>
                                </tr>
                            </tbody>
                        </table>
                        <p class="mt-3 text-xs opacity-60">
                            "Right-click selected text in the book to add a note or a bookmark."
                        </p>
                        <div class="modal-action">
                            <button class="btn btn-sm" on:click=move |_| set_show_shortcuts.set(false)>
                                "Close"
                            </button>
                        </div>
                    </div>
            </dialog>
        </main>
    }
}

#[component]
fn LibraryPanel(
    books: ReadSignal<Vec<StoredBook>>,
    filter: ReadSignal<String>,
    sort_mode: ReadSignal<String>,
    selected_id: ReadSignal<Option<String>>,
    set_filter: WriteSignal<String>,
    set_sort_mode: WriteSignal<String>,
    set_books: WriteSignal<Vec<StoredBook>>,
    import_file: impl Fn(File) + Copy + Send + Sync + 'static,
    open_book: impl Fn(StoredBook) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let visible_books = move || {
        let needle = filter.get().to_lowercase();
        books
            .get()
            .into_iter()
            .filter(|book| {
                needle.is_empty()
                    || book.record.title.to_lowercase().contains(&needle)
                    || book
                        .record
                        .creators
                        .iter()
                        .any(|creator| creator.to_lowercase().contains(&needle))
            })
            .collect::<Vec<_>>()
    };

    let file_input = NodeRef::<leptos::html::Input>::new();

    view! {
        <div class="p-4 border-b border-base-300">
            <div class="flex items-center gap-2">
                <Icon icon=icons::LuBookOpen width="1.25rem" height="1.25rem" style="color: var(--color-primary)" />
                <h1 class="font-semibold tracking-wide">"EPUB Reader RS"</h1>
            </div>
            <button
                class="btn btn-primary btn-sm mt-4 w-full"
                on:click=move |_| {
                    if let Some(input) = file_input.get() {
                        input.click();
                    }
                }
            >
                <Icon icon=icons::LuUpload width="1rem" height="1rem" />
                "Import EPUB"
            </button>
            <input
                node_ref=file_input
                class="hidden"
                type="file"
                accept=".epub,application/epub+zip"
                on:change=move |ev: Event| {
                    if let Some(input) = ev
                        .target()
                        .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
                    {
                        if let Some(file) = input.files().and_then(|files| files.get(0)) {
                            import_file(file);
                        }
                        input.set_value("");
                    }
                }
            />
        </div>

        <div class="p-4 space-y-3 border-b border-base-300">
            <label class="input input-sm input-bordered flex items-center gap-2">
                <Icon icon=icons::LuSearch width="1rem" height="1rem" style="opacity:.6" />
                <input
                    class="grow"
                    type="search"
                    aria-label="Search library by title or author"
                    placeholder="Search title or author"
                    prop:value=move || filter.get()
                    on:input=move |ev| set_filter.set(event_value(&ev))
                />
            </label>
            <select
                class="select select-sm select-bordered w-full"
                aria-label="Sort books"
                prop:value=move || sort_mode.get()
                on:change=move |ev| {
                    let mode = event_value(&ev);
                    set_sort_mode.set(mode.clone());
                    let mut next = books.get();
                    sort_books(&mut next, &mode);
                    set_books.set(next);
                }
            >
                <option value="recent">"Recent"</option>
                <option value="title">"Title"</option>
                <option value="author">"Author"</option>
                <option value="progress">"Progress"</option>
            </select>
        </div>

        <div class="min-h-0 flex-1 overflow-y-auto p-3 space-y-2">
            <Show
                when=move || !visible_books().is_empty()
                fallback=move || view! {
                    <div class="rounded-lg border border-dashed border-base-300 p-6 text-center">
                        <p class="text-sm font-medium">"No books yet"</p>
                        <p class="mt-1 text-xs opacity-60">
                            "Drop a DRM-free .epub file here, or press Import EPUB."
                        </p>
                    </div>
                }
            >
                <For
                    each=visible_books
                    key=|book| book.record.id.clone()
                    children=move |book| {
                        let book_for_open = book.clone();
                        let book_id = book.record.id.clone();
                        let cover = book
                            .record
                            .cover_href
                            .as_deref()
                            .and_then(|href| cover_data_uri(&book.bytes, href));
                        let initial = book
                            .record
                            .title
                            .chars()
                            .next()
                            .map(|c| c.to_uppercase().to_string())
                            .unwrap_or_else(|| "?".into());
                        view! {
                            <button
                                class="book-card"
                                class:book-card-active=move || selected_id.get() == Some(book_id.clone())
                                on:click=move |_| open_book(book_for_open.clone())
                            >
                                {match cover {
                                    Some(uri) => view! {
                                        <img class="book-cover" src=uri alt="" />
                                    }.into_any(),
                                    None => view! {
                                        <div class="book-cover book-cover-fallback" aria-hidden="true">
                                            {initial}
                                        </div>
                                    }.into_any(),
                                }}
                                <div class="min-w-0 flex-1">
                                    <p class="book-title truncate text-sm">{book.record.title.clone()}</p>
                                    <p class="mt-0.5 truncate text-xs opacity-60">
                                        {authors(&book.record)}
                                    </p>
                                    <div class="mt-2.5 flex items-center gap-2">
                                        <progress
                                            class="book-progress"
                                            value=book.record.progress.to_string()
                                            max="1"
                                        />
                                        <span class="text-[10px] tabular-nums opacity-60">
                                            {format!("{:.0}%", book.record.progress * 100.0)}
                                        </span>
                                    </div>
                                </div>
                            </button>
                        }
                    }
                />
            </Show>
        </div>
    }
}

#[component]
fn Toolbar(
    selected_book: Signal<Option<StoredBook>>,
    active_book: ReadSignal<Option<EpubBook>>,
    spine_index: ReadSignal<usize>,
    go_prev: impl Fn() + Copy + Send + Sync + 'static,
    go_next: impl Fn() + Copy + Send + Sync + 'static,
    toggle_zen: impl Fn() + Copy + Send + Sync + 'static,
    open_shortcuts: impl Fn() + Copy + Send + Sync + 'static,
    tts_state: ReadSignal<TtsUiState>,
    toggle_listen: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <header class="reader-toolbar h-14 px-6 flex items-center justify-between gap-3">
            <div class="min-w-0">
                <p class="book-title truncate text-sm">
                    {move || {
                        selected_book.get()
                            .map(|book| book.record.title)
                            .unwrap_or_else(|| "No book open".into())
                    }}
                </p>
                <p class="truncate text-xs opacity-55">
                    {move || {
                        selected_book.get()
                            .map(|book| authors(&book.record))
                            .unwrap_or_else(|| "Import or open an EPUB from the library".into())
                    }}
                </p>
            </div>
            <div class="flex items-center gap-1">
                <button
                    class="btn btn-ghost btn-sm btn-square"
                    aria-label="Previous chapter"
                    disabled=move || active_book.with(|b| b.is_none()) || spine_index.get() == 0
                    on:click=move |_| go_prev()
                >
                    <Icon icon=icons::LuChevronLeft width="1rem" height="1rem" />
                </button>
                <span class="min-w-20 text-center text-xs tabular-nums">
                    {move || {
                        active_book
                            .get()
                            .map(|book| format!("{}/{}", spine_index.get() + 1, book.package.spine.len()))
                            .unwrap_or_else(|| "-/-".into())
                    }}
                </span>
                <button
                    class="btn btn-ghost btn-sm btn-square"
                    aria-label="Next chapter"
                    disabled=move || {
                        active_book.with(|b| {
                            b.as_ref()
                                .is_none_or(|book| spine_index.get() + 1 >= book.package.spine.len())
                        })
                    }
                    on:click=move |_| go_next()
                >
                    <Icon icon=icons::LuChevronRight width="1rem" height="1rem" />
                </button>
                <div class="mx-1 h-5 w-px bg-base-300" aria-hidden="true"></div>
                <button
                    class="btn btn-ghost btn-sm btn-square"
                    class:text-primary=move || {
                        let state = tts_state.get();
                        state.is_active() || state == TtsUiState::Paused
                    }
                    aria-label="Read aloud"
                    disabled=move || active_book.with(|b| b.is_none())
                    title=move || match tts_state.get() {
                        TtsUiState::Speaking(..) => "Pause reading voice".to_string(),
                        TtsUiState::Paused => "Resume reading voice".to_string(),
                        TtsUiState::Downloading(frac) => {
                            format!("Downloading voice model ({:.0}%)", frac * 100.0)
                        }
                        _ => "Read aloud from this chapter".to_string(),
                    }
                    on:click=move |_| toggle_listen()
                >
                    {move || match tts_state.get() {
                        TtsUiState::Speaking(..) => view! { <span class="loading loading-bars loading-xs"></span> }.into_any(),
                        TtsUiState::Downloading(_) => view! { <span class="loading loading-spinner loading-xs"></span> }.into_any(),
                        _ => view! { <Icon icon=icons::LuHeadphones width="1rem" height="1rem" /> }.into_any(),
                    }}
                </button>
                <button class="btn btn-ghost btn-sm btn-square" aria-label="Full screen reading (F)" title="Full screen reading (F)" on:click=move |_| toggle_zen()>
                    <Icon icon=icons::LuMaximize2 width="1rem" height="1rem" />
                </button>
                <button class="btn btn-ghost btn-sm btn-square" aria-label="Keyboard shortcuts (?)" title="Keyboard shortcuts (?)" on:click=move |_| open_shortcuts()>
                    <Icon icon=icons::LuKeyboard width="1rem" height="1rem" />
                </button>
            </div>
        </header>
    }
}

#[component]
fn RightPanel(
    active_book: ReadSignal<Option<EpubBook>>,
    spine_index: ReadSignal<usize>,
    set_spine_index: WriteSignal<usize>,
    query: ReadSignal<String>,
    set_query: WriteSignal<String>,
    run_search: impl Fn() + Copy + Send + Sync + 'static,
    search_results: ReadSignal<Vec<SearchResult>>,
    preferences: ReadSignal<ReaderPreferences>,
    persist_preferences: impl Fn(ReaderPreferences) + Copy + Send + Sync + 'static,
    annotations: ReadSignal<Vec<Annotation>>,
    note_draft: ReadSignal<String>,
    set_note_draft: WriteSignal<String>,
    highlight_draft: ReadSignal<String>,
    set_highlight_draft: WriteSignal<String>,
    add_annotation: impl Fn() + Copy + Send + Sync + 'static,
    export_annotations: impl Fn() + Copy + Send + Sync + 'static,
    save_current_location: impl Fn() + Copy + Send + Sync + 'static,
    active_tab: ReadSignal<String>,
    set_active_tab: WriteSignal<String>,
    tts_state: ReadSignal<TtsUiState>,
    start_listen: impl Fn() + Copy + Send + Sync + 'static,
    pause_listen: impl Fn() + Copy + Send + Sync + 'static,
    stop_listen: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div class="tabs reader-tabs" role="group" aria-label="Reader panels">
            <input
                type="radio"
                name="reader_tabs"
                class="tab flex-1 min-w-0 px-1"
                aria-label="Contents"
                prop:checked=move || active_tab.get() == "toc"
                on:change=move |_| set_active_tab.set("toc".into())
            />
            <div class="tab-content p-3">
                <TocPanel
                    active_book=active_book
                    spine_index=spine_index
                    set_spine_index=set_spine_index
                    save_current_location=save_current_location
                />
            </div>

            <input
                type="radio"
                name="reader_tabs"
                class="tab flex-1 min-w-0 px-1"
                aria-label="Search"
                prop:checked=move || active_tab.get() == "search"
                on:change=move |_| set_active_tab.set("search".into())
            />
            <div class="tab-content p-3">
                <SearchPanel
                    query=query
                    set_query=set_query
                    run_search=run_search
                    search_results=search_results
                    set_spine_index=set_spine_index
                    save_current_location=save_current_location
                />
            </div>

            <input
                type="radio"
                name="reader_tabs"
                class="tab flex-1 min-w-0 px-1"
                aria-label="Notes"
                prop:checked=move || active_tab.get() == "notes"
                on:change=move |_| set_active_tab.set("notes".into())
            />
            <div class="tab-content p-3">
                <NotesPanel
                    annotations=annotations
                    note_draft=note_draft
                    set_note_draft=set_note_draft
                    highlight_draft=highlight_draft
                    set_highlight_draft=set_highlight_draft
                    add_annotation=add_annotation
                    export_annotations=export_annotations
                    active_book=active_book
                    set_spine_index=set_spine_index
                    save_current_location=save_current_location
                />
            </div>

            <input
                type="radio"
                name="reader_tabs"
                class="tab flex-1 min-w-0 px-1"
                aria-label="Style"
                prop:checked=move || active_tab.get() == "style"
                on:change=move |_| set_active_tab.set("style".into())
            />
            <div class="tab-content p-3">
                <PreferencesPanel preferences=preferences persist_preferences=persist_preferences />
            </div>

            <input
                type="radio"
                name="reader_tabs"
                class="tab flex-1 min-w-0 px-1"
                aria-label="Listen"
                prop:checked=move || active_tab.get() == "listen"
                on:change=move |_| set_active_tab.set("listen".into())
            />
            <div class="tab-content p-3">
                <ListenPanel
                    preferences=preferences
                    persist_preferences=persist_preferences
                    tts_state=tts_state
                    start_listen=start_listen
                    pause_listen=pause_listen
                    stop_listen=stop_listen
                />
            </div>
        </div>
    }
}

/// Pannello "Listen": voce locale Piper (VITS/onnxruntime-web, WASM). Il modello
/// voce viene scaricato da Hugging Face alla prima riproduzione e messo in cache.
#[component]
fn ListenPanel(
    preferences: ReadSignal<ReaderPreferences>,
    persist_preferences: impl Fn(ReaderPreferences) + Copy + Send + Sync + 'static,
    tts_state: ReadSignal<TtsUiState>,
    start_listen: impl Fn() + Copy + Send + Sync + 'static,
    pause_listen: impl Fn() + Copy + Send + Sync + 'static,
    stop_listen: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let status_line = move || match tts_state.get() {
        TtsUiState::Idle => "Ready.".to_string(),
        TtsUiState::Downloading(frac) => format!("Downloading voice model… {:.0}%", frac * 100.0),
        TtsUiState::Speaking(sentence, total) => {
            if total > 0 {
                format!("Reading aloud… sentence {sentence}/{total}")
            } else {
                "Reading aloud…".to_string()
            }
        }
        TtsUiState::Paused => "Paused.".to_string(),
        TtsUiState::Error(err) => format!("Error: {err}"),
    };

    view! {
        <div class="space-y-4">
            <h2 class="panel-label">"Read aloud"</h2>
            <p class="text-xs opacity-60">
                "Local neural voice (Piper, MIT). Audio is generated on this device; nothing is sent anywhere."
            </p>

            <label class="form-control">
                <span class="label-text">"Voice"</span>
                <select
                    class="select select-sm select-bordered"
                    prop:value=move || preferences.get().tts.language
                    on:change=move |ev| {
                        let mut next = preferences.get();
                        next.tts.language = event_value(&ev);
                        persist_preferences(next);
                    }
                >
                    {tts::VOICES
                        .iter()
                        .map(|voice| view! { <option value=voice.language>{voice.label}</option> })
                        .collect_view()}
                </select>
            </label>

            <label class="label cursor-pointer justify-start gap-3">
                <input
                    type="checkbox"
                    class="toggle toggle-sm"
                    prop:checked=move || preferences.get().tts.read_code_blocks
                    on:change=move |ev| {
                        let mut next = preferences.get();
                        next.tts.read_code_blocks = ev
                            .target()
                            .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
                            .is_some_and(|input| input.checked());
                        persist_preferences(next);
                    }
                />
                <span class="label-text">"Read code blocks aloud"</span>
            </label>

            <label class="form-control">
                <span class="label-text flex justify-between">
                    <span>"Speed"</span>
                    <span class="opacity-60 tabular-nums">
                        {move || format!("{:.2}×", preferences.get().tts.speed)}
                    </span>
                </span>
                <input
                    type="range"
                    class="range range-primary range-xs"
                    min="0.5"
                    max="2"
                    step="0.25"
                    prop:value=move || preferences.get().tts.speed
                    on:input=move |ev| {
                        if let Ok(speed) = event_value(&ev).parse::<f32>() {
                            let mut next = preferences.get();
                            next.tts.speed = speed;
                            persist_preferences(next);
                        }
                    }
                />
            </label>

            <Show when=move || matches!(tts_state.get(), TtsUiState::Downloading(_))>
                <progress
                    class="progress progress-primary w-full"
                    aria-label="Voice model download progress"
                    prop:value=move || match tts_state.get() {
                        TtsUiState::Downloading(frac) => frac,
                        _ => 0.0,
                    }
                    prop:max=1.0
                ></progress>
            </Show>

            <div class="flex items-center gap-1">
                <button
                    class="btn btn-sm btn-primary flex-1"
                    disabled=move || tts_state.get().is_active()
                    on:click=move |_| start_listen()
                >
                    <Icon icon=icons::LuPlay width=".9rem" height=".9rem" />
                    {move || if tts_state.get() == TtsUiState::Paused { "Resume" } else { "Play" }}
                </button>
                <button
                    class="btn btn-sm flex-1"
                    disabled=move || !matches!(tts_state.get(), TtsUiState::Speaking(..))
                    on:click=move |_| pause_listen()
                >
                    <Icon icon=icons::LuPause width=".9rem" height=".9rem" />
                    "Pause"
                </button>
                <button
                    class="btn btn-sm flex-1"
                    disabled=move || {
                        !(tts_state.get().is_active() || tts_state.get() == TtsUiState::Paused)
                    }
                    on:click=move |_| stop_listen()
                >
                    <Icon icon=icons::LuSquare width=".9rem" height=".9rem" />
                    "Stop"
                </button>
            </div>

            // Avanzamento della lettura: frase corrente sul totale del capitolo.
            <Show when=move || matches!(tts_state.get(), TtsUiState::Speaking(..))>
                <div class="flex items-center gap-2">
                    <span class="loading loading-bars loading-xs text-primary"></span>
                    <progress
                        class="progress progress-success w-full"
                        aria-label="Reading progress"
                        prop:value=move || match tts_state.get() {
                            TtsUiState::Speaking(sentence, _) => sentence as f64,
                            _ => 0.0,
                        }
                        prop:max=move || match tts_state.get() {
                            TtsUiState::Speaking(_, total) => total.max(1) as f64,
                            _ => 1.0,
                        }
                    ></progress>
                </div>
            </Show>

            <p class="text-xs opacity-60" role="status" aria-live="polite">{status_line}</p>
        </div>
    }
}

#[component]
fn TocPanel(
    active_book: ReadSignal<Option<EpubBook>>,
    spine_index: ReadSignal<usize>,
    set_spine_index: WriteSignal<usize>,
    save_current_location: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let toc_items = move || {
        active_book
            .get()
            .map(|book| {
                if book.package.toc.is_empty() {
                    book.package
                        .spine
                        .iter()
                        .enumerate()
                        .map(|(index, item)| TocItem {
                            label: format!("Chapter {}", index + 1),
                            href: item.href.clone(),
                            children: Vec::new(),
                        })
                        .collect()
                } else {
                    book.package.toc
                }
            })
            .unwrap_or_default()
    };

    view! {
        <div class="space-y-1">
            <For
                each=toc_items
                key=|item| item.href.clone()
                children=move |item| {
                    let active_href = item.href.clone();
                    let click_href = item.href.clone();
                    let target_for_active = move || {
                        active_book
                            .get()
                            .and_then(|book| spine_index_for_href(&book, &active_href))
                            .unwrap_or(spine_index.get())
                    };
                    view! {
                        <button
                            class="btn btn-ghost btn-sm w-full justify-start text-left"
                            class:btn-active=move || target_for_active() == spine_index.get()
                            on:click=move |_| {
                                let target = active_book
                                    .get()
                                    .and_then(|book| spine_index_for_href(&book, &click_href))
                                    .unwrap_or(spine_index.get());
                                set_spine_index.set(target);
                                save_current_location();
                            }
                        >
                            <span class="truncate">{item.label}</span>
                        </button>
                    }
                }
            />
        </div>
    }
}

#[component]
fn SearchPanel(
    query: ReadSignal<String>,
    set_query: WriteSignal<String>,
    run_search: impl Fn() + Copy + Send + Sync + 'static,
    search_results: ReadSignal<Vec<SearchResult>>,
    set_spine_index: WriteSignal<usize>,
    save_current_location: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div class="space-y-3">
            <div class="join w-full">
                <input
                    class="input input-sm input-bordered join-item min-w-0 flex-1"
                    type="search"
                    aria-label="Search in book"
                    placeholder="Search in book"
                    prop:value=move || query.get()
                    on:input=move |ev| set_query.set(event_value(&ev))
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        if ev.key() == "Enter" {
                            run_search();
                        }
                    }
                />
                <button class="btn btn-sm btn-primary join-item" aria-label="Search" on:click=move |_| run_search()>
                    <Icon icon=icons::LuSearch width="1rem" height="1rem" />
                </button>
            </div>
            <div class="space-y-2">
                <For
                    each=move || search_results.get()
                    key=|result| format!("{}:{}", result.spine_index, result.snippet)
                    children=move |result| view! {
                        <button
                            class="w-full rounded border border-base-300 bg-base-200 p-3 text-left hover:border-primary"
                            on:click=move |_| {
                                set_spine_index.set(result.spine_index);
                                save_current_location();
                            }
                        >
                            <p class="text-xs font-medium">{result.title}</p>
                            <p class="mt-1 text-xs opacity-75">{result.snippet}</p>
                        </button>
                    }
                />
            </div>
        </div>
    }
}

#[component]
fn NotesPanel(
    annotations: ReadSignal<Vec<Annotation>>,
    note_draft: ReadSignal<String>,
    set_note_draft: WriteSignal<String>,
    highlight_draft: ReadSignal<String>,
    set_highlight_draft: WriteSignal<String>,
    add_annotation: impl Fn() + Copy + Send + Sync + 'static,
    export_annotations: impl Fn() + Copy + Send + Sync + 'static,
    active_book: ReadSignal<Option<EpubBook>>,
    set_spine_index: WriteSignal<usize>,
    save_current_location: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div class="space-y-4">
            <div class="flex items-center justify-between">
                <h2 class="panel-label">"Notes & bookmarks"</h2>
                <button class="btn btn-ghost btn-xs" on:click=move |_| export_annotations()>
                    <Icon icon=icons::LuDownload width=".875rem" height=".875rem" />
                    "JSON"
                </button>
            </div>
            <input
                class="input input-sm input-bordered w-full"
                aria-label="Selected text or quote"
                placeholder="Selected text or quote"
                prop:value=move || highlight_draft.get()
                on:input=move |ev| set_highlight_draft.set(event_value(&ev))
            />
            <textarea
                class="textarea textarea-bordered min-h-24 w-full"
                aria-label="Note"
                placeholder="Note"
                prop:value=move || note_draft.get()
                on:input=move |ev| set_note_draft.set(textarea_value(&ev))
            />
            <button class="btn btn-primary btn-sm w-full" on:click=move |_| add_annotation()>
                <Icon icon=icons::LuHighlighter width="1rem" height="1rem" />
                "Save note"
            </button>
            <div class="space-y-2">
                <For
                    each=move || annotations.get()
                    key=|annotation| annotation.id.clone()
                    children=move |annotation| {
                        let is_bookmark = annotation.kind == AnnotationKind::Bookmark;
                        let href = annotation.spine_href.clone();
                        let quote = if annotation.selector.exact.is_empty() {
                            "Bookmark".to_string()
                        } else {
                            annotation.selector.exact.clone()
                        };
                        let note = annotation.note.clone();
                        view! {
                            <button
                                class="annotation-card"
                                on:click=move |_| {
                                    if let Some(book) = active_book.get()
                                        && let Some(index) = spine_index_for_href(&book, &href)
                                    {
                                        set_spine_index.set(index);
                                        save_current_location();
                                    }
                                }
                            >
                                <span class="annotation-kind">
                                    {if is_bookmark {
                                        view! { <Icon icon=icons::LuBookmark width=".7rem" height=".7rem" /> }
                                    } else {
                                        view! { <Icon icon=icons::LuHighlighter width=".7rem" height=".7rem" /> }
                                    }}
                                    {if is_bookmark { "Bookmark" } else { "Note" }}
                                </span>
                                <p class="mt-1 text-xs font-medium">{quote}</p>
                                <Show when={
                                    let note = note.clone();
                                    move || !note.is_empty()
                                }>
                                    <p class="mt-1 text-xs opacity-75">{note.clone()}</p>
                                </Show>
                            </button>
                        }
                    }
                />
            </div>
        </div>
    }
}

#[component]
fn PreferencesPanel(
    preferences: ReadSignal<ReaderPreferences>,
    persist_preferences: impl Fn(ReaderPreferences) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div class="space-y-4">
            <h2 class="panel-label">"Typography"</h2>
            <label class="form-control">
                <span class="label-text">"Theme"</span>
                <select
                    class="select select-sm select-bordered"
                    prop:value=move || theme_value(preferences.get().theme)
                    on:change=move |ev| {
                        let mut next = preferences.get();
                        next.theme = match event_value(&ev).as_str() {
                            "sepia" => ReaderTheme::Sepia,
                            "dark" => ReaderTheme::Dark,
                            _ => ReaderTheme::Light,
                        };
                        persist_preferences(next);
                    }
                >
                    <option value="light">"Light"</option>
                    <option value="sepia">"Sepia"</option>
                    <option value="dark">"Dark"</option>
                </select>
            </label>
            <label class="form-control">
                <span class="label-text">"Font"</span>
                <select
                    class="select select-sm select-bordered"
                    prop:value=move || font_value(preferences.get().font_family)
                    on:change=move |ev| {
                        let mut next = preferences.get();
                        next.font_family = if event_value(&ev) == "sans" {
                            ReaderFont::Sans
                        } else {
                            ReaderFont::Serif
                        };
                        persist_preferences(next);
                    }
                >
                    <option value="serif">"Serif"</option>
                    <option value="sans">"Sans"</option>
                </select>
            </label>
            <RangeControl
                label="Font size"
                value=move || preferences.get().font_size.to_string()
                min="14"
                max="28"
                step="1"
                on_input=move |value| {
                    let mut next = preferences.get();
                    next.font_size = value.parse().unwrap_or(next.font_size);
                    persist_preferences(next);
                }
            />
            <RangeControl
                label="Line height"
                value=move || preferences.get().line_height.to_string()
                min="1.2"
                max="2.2"
                step="0.05"
                on_input=move |value| {
                    let mut next = preferences.get();
                    next.line_height = value.parse().unwrap_or(next.line_height);
                    persist_preferences(next);
                }
            />
            <RangeControl
                label="Page width"
                value=move || preferences.get().page_width.to_string()
                min="560"
                max="980"
                step="20"
                on_input=move |value| {
                    let mut next = preferences.get();
                    next.page_width = value.parse().unwrap_or(next.page_width);
                    persist_preferences(next);
                }
            />
            <RangeControl
                label="Margins"
                value=move || preferences.get().page_margin.to_string()
                min="16"
                max="96"
                step="4"
                on_input=move |value| {
                    let mut next = preferences.get();
                    next.page_margin = value.parse().unwrap_or(next.page_margin);
                    persist_preferences(next);
                }
            />
            <label class="form-control">
                <span class="label-text">"Flow"</span>
                <select
                    class="select select-sm select-bordered"
                    prop:value=move || flow_value(preferences.get().flow)
                    on:change=move |ev| {
                        let mut next = preferences.get();
                        next.flow = if event_value(&ev) == "paged" {
                            ReaderFlow::Paged
                        } else {
                            ReaderFlow::Scroll
                        };
                        persist_preferences(next);
                    }
                >
                    <option value="scroll">"Scroll"</option>
                    <option value="paged">"Paged"</option>
                </select>
            </label>
            <label class="label cursor-pointer justify-start gap-3">
                <input
                    type="checkbox"
                    class="toggle toggle-sm"
                    prop:checked=move || preferences.get().publisher_styles
                    on:change=move |ev| {
                        let mut next = preferences.get();
                        next.publisher_styles = ev
                            .target()
                            .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
                            .is_some_and(|input| input.checked());
                        persist_preferences(next);
                    }
                />
                <span class="label-text">"Publisher styles"</span>
            </label>
        </div>
    }
}

#[component]
fn RangeControl(
    label: &'static str,
    value: impl Fn() -> String + Copy + Send + Sync + 'static,
    min: &'static str,
    max: &'static str,
    step: &'static str,
    on_input: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <label class="form-control">
            <div class="label">
                <span class="label-text">{label}</span>
                <span class="label-text-alt tabular-nums">{value}</span>
            </div>
            <input
                class="range range-primary range-xs"
                type="range"
                min=min
                max=max
                step=step
                prop:value=value
                on:input=move |ev| on_input(event_value(&ev))
            />
        </label>
    }
}

async fn read_file_bytes(file: File) -> Result<Vec<u8>, String> {
    let buffer = JsFuture::from(file.array_buffer())
        .await
        .map_err(|_| "Could not read the EPUB file.".to_string())?;
    let array = js_sys::Uint8Array::new(&buffer);
    let mut bytes = vec![0; array.length() as usize];
    array.copy_to(&mut bytes);
    Ok(bytes)
}

fn import_book(bytes: Vec<u8>) -> Result<StoredBook, String> {
    let epub = EpubBook::from_bytes(&bytes).map_err(|err| err.to_string())?;
    let now = js_sys::Date::now();
    let record = BookRecord {
        id: stable_book_id(&epub, &bytes),
        title: epub.package.metadata.title.clone(),
        creators: epub.package.metadata.creators.clone(),
        language: epub.package.metadata.language.clone(),
        cover_href: cover_href(&epub),
        added_at: now,
        last_opened_at: Some(now),
        progress: 0.0,
    };
    Ok(StoredBook {
        record,
        package: epub.package,
        bytes,
    })
}

/// Stable id so re-importing the same publication replaces it instead of
/// piling up duplicates: prefer the unique OPF identifier, fall back to a
/// content hash when the EPUB declares none.
fn stable_book_id(epub: &EpubBook, bytes: &[u8]) -> String {
    if let Some(id) = epub
        .package
        .metadata
        .identifier
        .as_ref()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
    {
        return format!("id:{id}");
    }
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("hash:{:016x}", hasher.finish())
}

fn sort_books(books: &mut [StoredBook], mode: &str) {
    match mode {
        "title" => books.sort_by_key(|book| book.record.title.to_lowercase()),
        "author" => books.sort_by_key(|book| authors(&book.record).to_lowercase()),
        "progress" => books.sort_by(|a, b| {
            b.record
                .progress
                .partial_cmp(&a.record.progress)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => books.sort_by(|a, b| {
            b.record
                .last_opened_at
                .unwrap_or(b.record.added_at)
                .partial_cmp(&a.record.last_opened_at.unwrap_or(a.record.added_at))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }
}

fn spine_index_for_href(book: &EpubBook, href: &str) -> Option<usize> {
    let href = href.split('#').next().unwrap_or(href);
    book.package
        .spine
        .iter()
        .position(|item| item.href == href || item.href.ends_with(href))
}

fn authors(record: &BookRecord) -> String {
    if record.creators.is_empty() {
        "Unknown author".into()
    } else {
        record.creators.join(", ")
    }
}

fn cover_href(book: &EpubBook) -> Option<String> {
    book.package
        .manifest
        .values()
        .find(|item| {
            item.properties
                .iter()
                .any(|property| property == "cover-image")
                || item.id.to_lowercase().contains("cover")
        })
        .map(|item| item.href.clone())
}

fn event_value(ev: &Event) -> String {
    event_target_value(ev)
}

fn textarea_value(ev: &Event) -> String {
    event_target_value(ev)
}

fn selection_text_from_iframe(iframe_ref: NodeRef<Iframe>) -> Option<String> {
    let iframe = iframe_ref.get_untracked()?;
    let selection = iframe.content_window()?.get_selection().ok()??;
    Some(selection.to_string().into())
}

fn iframe_body(iframe_ref: NodeRef<Iframe>) -> Option<web_sys::HtmlElement> {
    iframe_ref.get_untracked()?.content_document()?.body()
}

/// Testo del capitolo a partire dal primo elemento con testo *visibile* a schermo
/// nell'iframe (per far partire la lettura dalla pagina mostrata, non dall'inizio
/// del capitolo). Usa `elementFromPoint` in coordinate viewport, quindi funziona sia
/// in scroll che a colonne. `None` se non si trova nulla → il chiamante legge tutto.
fn visible_chapter_text(iframe_ref: NodeRef<Iframe>) -> Option<String> {
    let doc = iframe_ref.get_untracked()?.content_document()?;
    let body = doc.body()?;
    let x = (doc.document_element()?.client_width() as f32) * 0.5;
    // Campiona qualche y vicino al bordo alto: salta margini/immagini vuote.
    let start = [10.0_f32, 30.0, 60.0, 100.0, 150.0].into_iter().find_map(|y| {
        let el = doc.element_from_point(x, y)?;
        let tag = el.tag_name().to_ascii_lowercase();
        if tag == "body" || tag == "html" {
            return None;
        }
        if el.text_content().unwrap_or_default().trim().is_empty() {
            return None;
        }
        Some(el)
    })?;
    let range = doc.create_range().ok()?;
    range.select_node_contents(&body).ok()?;
    range.set_start_before(&start).ok()?;
    let text: String = range.to_string().into();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

fn set_browser_fullscreen(enable: bool) {
    let Some(doc) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    if enable {
        if let Some(root) = doc.document_element() {
            let _ = root.request_fullscreen();
        }
    } else if doc.fullscreen_element().is_some() {
        doc.exit_fullscreen();
    }
}

fn copy_to_clipboard(text: &str) {
    if let Some(window) = web_sys::window() {
        let navigator = window.navigator();
        let clipboard = navigator.clipboard();
        let _ = clipboard.write_text(text);
    }
}

fn theme_value(theme: ReaderTheme) -> &'static str {
    match theme {
        ReaderTheme::Light => "light",
        ReaderTheme::Sepia => "sepia",
        ReaderTheme::Dark => "dark",
    }
}

fn font_value(font: ReaderFont) -> &'static str {
    match font {
        ReaderFont::Serif => "serif",
        ReaderFont::Sans => "sans",
    }
}

fn flow_value(flow: ReaderFlow) -> &'static str {
    match flow {
        ReaderFlow::Scroll => "scroll",
        ReaderFlow::Paged => "paged",
    }
}

fn empty_reader_doc(preferences: &ReaderPreferences) -> String {
    error_reader_doc(
        "Open a book from the library on the left, or drop an .epub file here.",
        &reader_css(preferences),
    )
}

fn error_reader_doc(message: &str, css: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>{css}</style></head><body><main><h1>EPUB Reader RS</h1><p>{}</p></main></body></html>"#,
        message
    )
}
