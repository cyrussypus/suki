mod account;
mod api;
mod checkout;
mod gps;
mod market;
mod orders;
mod staff;
mod vendor;

use api::Api;
use dioxus::prelude::*;
use suki_shared::{
    admin::Settings,
    auth::{Account, Role, SessionAccount},
    catalog::{BasketLine, Favorites, Product},
};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Market,
    Favorites,
    Basket,
    Orders,
    Messages,
    Account,
    Dashboard,
}

#[derive(Clone, Copy)]
struct State {
    page: Signal<Page>,
    account: Signal<Option<Account>>,
    admin_access: Signal<bool>,
    signing_out: Signal<bool>,
    catalog: Signal<Vec<Product>>,
    categories: Signal<Vec<String>>,
    settings: Signal<Option<Settings>>,
    basket: Signal<Vec<BasketLine>>,
    pending: Signal<Option<suki_shared::orders::Checkout>>,
    favorites: Signal<Favorites>,
    active_order: Signal<Option<String>>,
    notice: Signal<String>,
}

fn can_open_dashboard(account: &Account, admin_access: bool) -> bool {
    account.has_role(Role::Vendor)
        || account.has_role(Role::Rider)
        || (account.has_role(Role::Admin) && admin_access)
}

fn home_for(account: &Account, admin_access: bool) -> Page {
    if can_open_dashboard(account, admin_access) {
        Page::Dashboard
    } else if account.has_role(Role::Customer) {
        Page::Market
    } else {
        Page::Account
    }
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
        admin_access: use_signal(|| false),
        signing_out: use_signal(|| false),
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
            if let Ok(SessionAccount {
                account,
                admin_access,
            }) = api.get("/auth/me").await
            {
                state.admin_access.set(admin_access);
                if (state.page)() == Page::Market {
                    state.page.set(home_for(&account, admin_access));
                }
                state.account.set(Some(account));
            }
        }
    });
    let page = (state.page)();
    let active = (state.active_order)();
    let basket_count = state.basket.read().len();
    let account = (state.account)();
    let customer = account.as_ref().is_none_or(|a| a.has_role(Role::Customer));
    let admin_access = (state.admin_access)();
    let dashboard = account
        .as_ref()
        .is_some_and(|a| can_open_dashboard(a, admin_access));
    let vendor = account.as_ref().is_some_and(|a| a.has_role(Role::Vendor));
    rsx! {
        document::Title { "Suki Davao" }
        document::Link { rel: "icon", r#type: "image/png", href: asset!("/assets/suki-icon.png") }
        style { {include_str!("style.css")} }
        header {
            div { class: "brand", img { class: "brand-logo", src: asset!("/assets/suki-logo.png"), alt: "Suki" } small { "YOUR DAVAO MARKET" } }
            if customer {
                button { class: "basket-shortcut", aria_label: "Open basket, {basket_count} items", onclick: move |_| state.page.set(Page::Basket),
                    svg { view_box: "0 0 24 24", path { d: "M4 9h16l-1 11H5L4 9zM8 9V7a4 4 0 0 1 8 0v2" } } " Basket" if basket_count > 0 { b { "{basket_count}" } }
                }
            } else {
                button { class: "quiet", onclick: move |_| state.page.set(Page::Account), "Account" }
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
                    rsx! { orders::OrderList { messages_only: false } }
                },
                Page::Messages => if let Some(id) = active {
                    rsx! { orders::OrderScreen { key: "{id}", id } }
                } else {
                    rsx! { orders::OrderList { messages_only: true } }
                },
                Page::Account => rsx! { account::AccountScreen {} },
                Page::Dashboard => match &account {
                    Some(a) if a.has_role(Role::Vendor) => rsx! { vendor::VendorDashboard { vendor_id: a.id.clone() } },
                    Some(a) if a.has_role(Role::Admin) && admin_access => rsx! { staff::AdminDashboard {} },
                    Some(a) if a.has_role(Role::Rider) => rsx! { orders::OrderList { messages_only: false } },
                    _ => rsx! { account::AccountScreen {} },
                },
            }
        }
        nav { aria_label: "Main navigation",
            if customer {
                button { aria_current: if page == Page::Market { "page" }, onclick: move |_| state.page.set(Page::Market),
                    svg { view_box: "0 0 24 24", path { d: "M3 10h18M5 10v10h14V10M4 10l2-6h12l2 6M9 20v-6h6v6" } } span { "Market" }
                }
            } else if dashboard {
                button { aria_current: if page == Page::Dashboard { "page" }, onclick: move |_| state.page.set(Page::Dashboard),
                    svg { view_box: "0 0 24 24", path { d: "M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z" } } span { "Dashboard" }
                }
            }
            if !vendor {
                button { aria_current: if page == Page::Messages { "page" }, onclick: move |_| {
                    state.active_order.set(None); state.page.set(Page::Messages);
                }, svg { view_box: "0 0 24 24", path { d: "M20 11.5a8 8 0 0 1-8 8 8.5 8.5 0 0 1-3.5-.7L4 20l1.2-4A8 8 0 1 1 20 11.5zM8 11.5h.01M12 11.5h.01M16 11.5h.01" } } span { "Messages" } }
                button { aria_current: if page == Page::Orders { "page" }, onclick: move |_| {
                    state.active_order.set(None); state.page.set(Page::Orders);
                }, svg { view_box: "0 0 24 24", path { d: "M6 4h12v16H6zM9 8h6M9 12h6M9 16h4" } } span { "Orders" } }
            }
            button { aria_current: if page == Page::Account { "page" }, onclick: move |_| state.page.set(Page::Account),
                svg { view_box: "0 0 24 24", path { d: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4.5 20a7.5 7.5 0 0 1 15 0" } } span { "Account" }
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
