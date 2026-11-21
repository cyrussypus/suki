mod account;
mod api;
mod checkout;
mod market;
mod orders;

use api::Api;
use dioxus::prelude::*;
use suki_shared::{
    admin::Settings,
    auth::{Account, Role},
    catalog::{BasketLine, Favorites, Product},
};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Market,
    Favorites,
    Basket,
    Orders,
    Account,
}

#[derive(Clone, Copy)]
struct State {
    page: Signal<Page>,
    account: Signal<Option<Account>>,
    catalog: Signal<Vec<Product>>,
    categories: Signal<Vec<String>>,
    settings: Signal<Option<Settings>>,
    basket: Signal<Vec<BasketLine>>,
    pending: Signal<Option<suki_shared::orders::Checkout>>,
    favorites: Signal<Favorites>,
    active_order: Signal<Option<String>>,
    notice: Signal<String>,
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let api = use_hook(Api::new);
    let mut state = State {
        page: use_signal(|| Page::Market),
        account: use_signal(|| None),
        catalog: use_signal(Vec::new),
        categories: use_signal(Vec::new),
        settings: use_signal(|| None),
        basket: use_signal(Vec::new),
        pending: use_signal(|| None),
        favorites: use_signal(Favorites::new),
        active_order: use_signal(|| None),
        notice: use_signal(String::new),
    };
    let api = match api {
        Ok(api) => api,
        Err(error) => return rsx! { main { role: "alert", "Suki could not start: {error}" } },
    };
    use_context_provider(|| api.clone());
    use_context_provider(|| state);
    use_future(move || {
        let api = api.clone();
        async move {
            refresh_catalog(&api, state).await;
            if let Ok(account) = api.get::<Account>("/auth/me").await {
                state.account.set(Some(account));
            }
        }
    });
    let page = (state.page)();
    let active = (state.active_order)();
    let basket_count = state.basket.read().len();
    let signed_in = state.account.read().is_some();
    let vendor = state
        .account
        .read()
        .as_ref()
        .is_some_and(|a| a.has_role(Role::Vendor));
    rsx! {
        style { {include_str!("style.css")} }
        header {
            div { class: "brand", span { "S" } div { strong { "Suki" } small { "DAVAO CITY MARKET" } } }
            button { class: "quiet", onclick: move |_| state.page.set(Page::Account),
                if signed_in { "Account" } else { "Sign in" }
            }
        }
        main {
            if !(state.notice)().is_empty() {
                div { class: "notice global", role: "status",
                    span { "{state.notice}" }
                    button { class: "quiet", aria_label: "Dismiss notice", onclick: move |_| state.notice.set(String::new()), "×" }
                }
            }
            match page {
                Page::Market => rsx! { market::Market { favorites_only: false } },
                Page::Favorites => rsx! { market::Market { favorites_only: true } },
                Page::Basket => rsx! { checkout::Basket {} },
                Page::Orders => if let Some(id) = active {
                    rsx! { orders::OrderScreen { key: "{id}", id } }
                } else {
                    rsx! { orders::OrderList {} }
                },
                Page::Account => rsx! { account::AccountScreen {} },
            }
        }
        nav { aria_label: "Main navigation",
            button { aria_current: if page == Page::Market { "page" }, onclick: move |_| state.page.set(Page::Market), "Market" }
            button { aria_current: if page == Page::Favorites { "page" }, onclick: move |_| state.page.set(Page::Favorites), "Favorites" }
            button { aria_current: if page == Page::Basket { "page" }, onclick: move |_| state.page.set(Page::Basket), "Basket ({basket_count})" }
            if !vendor {
                button { aria_current: if page == Page::Orders { "page" }, onclick: move |_| {
                    state.active_order.set(None); state.page.set(Page::Orders);
                }, "Orders" }
            }
        }
    }
}

async fn refresh_catalog(api: &Api, mut state: State) {
    match api.get("/products").await {
        Ok(products) => state.catalog.set(products),
        Err(error) => state.notice.set(error.to_string()),
    }
    match api.get("/categories").await {
        Ok(categories) => state.categories.set(categories),
        Err(error) => state.notice.set(error.to_string()),
    }
    match api.get("/settings").await {
        Ok(settings) => state.settings.set(Some(settings)),
        Err(error) => state.notice.set(error.to_string()),
    }
}

fn now() -> i64 {
    #[cfg(target_arch = "wasm32")]
    return (js_now() / 1000.0) as i64;
    #[cfg(not(target_arch = "wasm32"))]
    return std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
}

#[cfg(target_arch = "wasm32")]
fn js_now() -> f64 {
    js_sys::Date::now()
}
