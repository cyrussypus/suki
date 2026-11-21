use crate::{Api, State};
use dioxus::prelude::*;
use suki_shared::{
    catalog::{BasketLine, CatalogFilter, Product},
    pricing::{MAX_QUANTITY_MILLI, peso},
};

#[component]
pub fn Market(favorites_only: bool) -> Element {
    let api = use_context::<Api>();
    let state = use_context::<State>();
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
                p { class: "eyebrow", "BANKEROHAN PUBLIC MARKET" }
                h1 { "Fresh from Davao." }
                p { "Shop your neighborhood market and get your fresh picks delivered." }
            }
        }
        section { class: "market",
            div { class: "line heading",
                h2 { if favorites_only { "Your favorites" } else { "Shop the market" } }
                button { class: "quiet", disabled: refreshing(), onclick: move |_| {
                    let api = api.clone();
                    refreshing.set(true);
                    async move { crate::refresh_catalog(&api, state).await; refreshing.set(false); }
                }, "Refresh" }
            }
            div { class: "line filters",
                input { r#type: "search", aria_label: "Search products", placeholder: "Search mangoes, fish, vendors…", value: "{chosen.search}", oninput: move |e| filter.write().search = e.value() }
                button { aria_expanded: drawer(), onclick: move |_| drawer.toggle(), "Filters" }
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
            if filtered.is_empty() {
                p { class: "empty", if favorites_only { "No matching favorites. Save products with the heart button." } else { "No products match these filters." } }
            }
            div { class: "product-grid",
                for product in filtered { ProductCard { key: "{product.id}", product } }
            }
        }
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
                p { "{product.description}" }
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
