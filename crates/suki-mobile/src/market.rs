use crate::{Api, Page, State};
use dioxus::prelude::*;
use suki_shared::{
    catalog::{BasketLine, CatalogFilter, Product},
    pricing::{MAX_QUANTITY_MILLI, peso},
};

#[component]
pub fn Market(favorites_only: bool) -> Element {
    let api = use_context::<Api>();
    let mut state = use_context::<State>();
    let mut filter = use_signal(CatalogFilter::default);
    let mut drawer = use_signal(|| false);
    let mut refreshing = use_signal(|| false);
    let chosen = filter();
    let favorites = state.favorites.read();
    let catalog = state.catalog.read();
    let vendors: std::collections::BTreeMap<_, _> =
        catalog.iter().map(|p| (&p.vendor_id, &p.vendor)).collect();
    let filtered = catalog
        .iter()
        .filter(|p| (!favorites_only || favorites.contains(&p.id)) && chosen.matches(p))
        .cloned()
        .collect::<Vec<_>>();
    let hero = api.image("/images/hero.jpg");
    rsx! {
        if !favorites_only {
            section { class: "hero", style: hero.map(|path| format!("--hero: url('{path}')")),
                div { class: "hero-copy",
                    p { class: "eyebrow", "YOUR NEIGHBORHOOD MARKET" }
                    h1 { "A little closer to fresh." }
                    p { "Good food, good people, straight from Davao’s market." }
                    span { class: "hero-location", "✦  From Bankerohan, with care" }
                }
            }
        }
        section { class: "market",
            div { class: "market-intro",
                div { p { class: "eyebrow", if favorites_only { "SAVED FOR LATER" } else { "FRESH FINDS, EVERY DAY" } }
                    h2 { if favorites_only { "Your favorites" } else { "What’s on your list?" } }
                }
                if !favorites_only {
                    button { class: "favorites-link", onclick: move |_| state.page.set(Page::Favorites), "♡  Favorites" }
                } else {
                    button { class: "quiet", onclick: move |_| state.page.set(Page::Market), "← Market" }
                }
            }
            div { class: "search-row",
                span { class: "search-icon", aria_hidden: "true", "⌕" }
                input { r#type: "search", aria_label: "Search products", placeholder: "Search produce, stalls and more", value: "{chosen.search}", oninput: move |e| filter.write().search = e.value() }
                button { class: "filter-button", aria_expanded: drawer(), onclick: move |_| drawer.toggle(), "Filters" }
            }
            if !favorites_only {
                div { class: "section-heading", h3 { "Explore the market" } span { "Browse by category" } }
                div { class: "category-list", aria_label: "Product categories",
                    button { class: if chosen.category.is_none() { "category active" } else { "category" }, onclick: move |_| filter.write().category = None,
                        span { class: "category-icon", "✳" } span { "All finds" }
                    }
                    for category in state.categories.read().iter().cloned() {
                        {
                            let selected = chosen.category.as_deref() == Some(category.as_str());
                            let icon = category_icon(&category);
                            let category_value = category.clone();
                            rsx! { button { key: "{category}", class: if selected { "category active" } else { "category" },
                                onclick: move |_| filter.write().category = Some(category_value.clone()),
                                span { class: "category-icon", "{icon}" } span { "{category}" }
                            } }
                        }
                    }
                }
            }
            if drawer() {
                div { class: "panel filter-drawer",
                    label { "Category"
                        select { value: chosen.category.clone().unwrap_or_default(), onchange: move |e| filter.write().category = nonempty(e.value()),
                            option { value: "", "All categories" }
                            for category in state.categories.read().iter() {
                                option { key: "{category}", value: "{category}", "{category}" }
                            }
                        }
                    }
                    label { "Vendor"
                        select { value: chosen.vendor_id.clone().unwrap_or_default(), onchange: move |e| filter.write().vendor_id = nonempty(e.value()),
                            option { value: "", "All vendors" }
                            for (id, name) in vendors { option { key: "{id}", value: "{id}", "{name}" } }
                        }
                    }
                }
            }
            div { class: "section-heading picks-heading", h3 { if favorites_only { "Saved picks" } else { "Fresh from the stalls" } }
                button { class: "quiet refresh-link", disabled: refreshing(), onclick: move |_| {
                    let api = api.clone(); refreshing.set(true);
                    async move { crate::refresh_catalog(&api, state).await; refreshing.set(false); }
                }, "Refresh ↻" }
            }
            if filtered.is_empty() {
                div { class: "empty market-empty", span { class: "empty-icon", "✳" }
                    h3 { if favorites_only { "Nothing saved yet" } else { "No finds just yet" } }
                    p { if favorites_only { "Tap the heart on a product to save it here." } else { "Try another search or category, or check back for fresh picks." } }
                }
            }
            div { class: "product-grid",
                for product in filtered { ProductCard { key: "{product.id}", product } }
            }
        }
    }
}

fn category_icon(category: &str) -> &'static str {
    match category {
        "Vegetables" => "🥬",
        "Fruits" => "🍊",
        "Meat & Poultry" => "🍗",
        "Fish & Seafood" => "🐟",
        _ => "🧺",
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[component]
fn ProductCard(product: Product) -> Element {
    let api = use_context::<Api>();
    let mut state = use_context::<State>();
    let id = product.id;
    let price = product.price_centavos;
    let favorites = state.favorites.read();
    let favorite = favorites.contains(&id);
    let photo = api.image(&product.image);
    rsx! {
        article { class: "product-card",
            div { class: "product-photo",
                if let Some(photo) = photo { img { src: "{photo}", alt: "", loading: "lazy" } }
                button { class: "heart", aria_pressed: favorite,
                    aria_label: if favorite { "Remove {product.name} from favorites" } else { "Save {product.name} to favorites" },
                    onclick: move |_| {
                        let mut favorites = state.favorites.write();
                        if !favorites.remove(&id) { favorites.insert(id); }
                    },
                    if favorite { "♥" } else { "♡" }
                }
            }
            div { class: "product-info",
                small { "{product.vendor}" }
                h3 { "{product.name}" }
                if !product.local_name.is_empty() { p { class: "muted", "{product.local_name}" } }

                div { class: "line",
                    span { strong { "{peso(price)}" } " / {product.unit}" }
                    button { class: "primary", disabled: !product.available || state.pending.read().is_some(),
                        aria_label: "Add {product.name} to basket",
                        onclick: move |_| {
                            let has_other_vendor = state.basket.read().iter().any(|item| {
                                state.catalog.read().iter().any(|p| p.id == item.product_id && p.vendor_id != product.vendor_id)
                            });
                            if has_other_vendor {
                                state.notice.set("Finish this stall's basket before adding products from another vendor.".into());
                                return;
                            }
                            let mut basket = state.basket.write();
                            if let Some(item) = basket.iter_mut().find(|item| item.product_id == id) {
                                item.quantity_milli = (item.quantity_milli + 1000).min(MAX_QUANTITY_MILLI);
                            } else {
                                basket.push(BasketLine { product_id: id, price_centavos: price, quantity_milli: 1000 });
                            }
                        },
                        if product.available { "+" } else { "Sold out" }
                    }
                }
            }
        }
    }
}
