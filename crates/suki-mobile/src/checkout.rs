use crate::{Api, Page, State, now};
use dioxus::prelude::*;
use reqwest::Method;
use suki_shared::{
    addresses::Address,
    auth::Role,
    catalog::BasketLine,
    delivery::{DeliveryQuote, Destination},
    orders::{Checkout, CheckoutItem, CheckoutResult},
    pricing::{MAX_QUANTITY_MILLI, cost, permits_goods_subtotal, peso},
};

#[component]
pub fn Basket() -> Element {
    let api = use_context::<Api>();
    let mut state = use_context::<State>();
    let mut address = use_signal(String::new);
    let mut latitude = use_signal(String::new);
    let mut longitude = use_signal(String::new);
    let mut quote = use_signal(|| None::<DeliveryQuote>);
    let mut addresses = use_signal(Vec::<Address>::new);
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let customer = state
        .account
        .read()
        .as_ref()
        .is_some_and(|a| a.has_role(Role::Customer));
    let signed_in = state.account.read().is_some();
    use_future({
        let api = api.clone();
        move || {
            let api = api.clone();
            async move {
                if customer && let Ok(saved) = api.get("/addresses").await {
                    addresses.set(saved);
                }
            }
        }
    });

    let lines = (state.basket)();
    let catalog = state.catalog.read();
    let settings = (state.settings)();
    let subtotal: i64 = lines
        .iter()
        .filter_map(|line| cost(line.price_centavos, line.quantity_milli))
        .sum();
    let pending = (state.pending)();
    let allow_edit = !busy() && pending.is_none();
    let minimum_met = settings
        .as_ref()
        .is_some_and(|s| permits_goods_subtotal(subtotal, s.minimum_centavos));
    let has_changes = lines.iter().any(|line| {
        !catalog.iter().any(|product| {
            product.id == line.product_id
                && product.available
                && product.price_centavos == line.price_centavos
        })
    });
    let quoted = (quote)();
    let can_checkout = minimum_met
        && !has_changes
        && !lines.is_empty()
        && settings
            .as_ref()
            .is_some_and(|s| !s.gcash_qr_url.is_empty())
        && quoted.as_ref().is_some_and(|q| q.expires_at > now());
    let quote_api = api.clone();
    rsx! {
        section { class: "panel", h1 { "Your basket" }
            if lines.is_empty() { p { "Your basket is empty. Find something fresh at the market." } }
            for line in lines.iter().cloned() {
                BasketRow { key: "{line.product_id}", line: line.clone(), editable: allow_edit }
            }
            div { class: "line totals", span { "Goods subtotal" } strong { "{peso(subtotal)}" } }
            if let Some(settings) = settings {
                p { class: "muted", "Minimum goods order: {peso(settings.minimum_centavos)}. Delivery is separate." }
                if settings.gcash_qr_url.is_empty() {
                    p { class: "notice", "Checkout is unavailable until the operator configures GCash." }
                }
            }
            if has_changes && allow_edit {
                p { class: "notice", "The catalog changed. Review affected items before you continue." }
            }
        }
        if !signed_in {
            section { class: "panel", button { class: "primary", onclick: move |_| state.page.set(Page::Account), "Sign in to check out" } }
        } else if !customer {
            section { class: "panel", p { "Checkout is available to customer accounts." } }
        } else if !lines.is_empty() || pending.is_some() {
            section { class: "panel",
                h2 { "Delivery and checkout" }
                if pending.is_none() {
                    form {
                        onsubmit: move |event| {
                            event.prevent_default();
                            if busy() { return; }
                            let (Ok(lat), Ok(lng)) = (latitude().parse::<f64>(), longitude().parse::<f64>()) else {
                                error.set("Enter valid latitude and longitude.".into());
                                return;
                            };
                            if !lat.is_finite() || !lng.is_finite() {
                                error.set("Enter valid latitude and longitude.".into());
                                return;
                            }
                            let destination = Destination { address: address(), lat, lng };
                            let api = quote_api.clone();
                            quote.set(None);
                            busy.set(true);
                            error.set(String::new());
                            spawn(async move {
                                match api.send(Method::POST, "/delivery/quote", &destination).await {
                                    Ok(result) => quote.set(Some(result)),
                                    Err(failure) => error.set(failure.to_string()),
                                }
                                busy.set(false);
                            });
                        },
                        fieldset { disabled: !allow_edit,
                            if !addresses.read().is_empty() {
                                label { "Saved addresses"
                                    select { onchange: move |event| {
                                        if let Some(saved) = addresses.read().iter().find(|saved| saved.id == event.value()) {
                                            address.set(saved.address.clone());
                                            latitude.set(saved.lat.to_string());
                                            longitude.set(saved.lng.to_string());
                                            quote.set(None);
                                        }
                                    },
                                        option { value: "", "Choose an address" }
                                        for saved in addresses.read().iter() {
                                            option { key: "{saved.id}", value: "{saved.id}", "{saved.label}" }
                                        }
                                    }
                                }
                            }
                            label { "Delivery address" input { required: true, minlength: "10", maxlength: "500", value: "{address}", oninput: move |e| { address.set(e.value()); quote.set(None); } } }
                            div { class: "field-pair",
                                label { "Latitude" input { required: true, inputmode: "decimal", value: "{latitude}", oninput: move |e| { latitude.set(e.value()); quote.set(None); } } }
                                label { "Longitude" input { required: true, inputmode: "decimal", value: "{longitude}", oninput: move |e| { longitude.set(e.value()); quote.set(None); } } }
                            }
                            button { r#type: "submit", "Get road distance quote" }
                        }
                    }
                    if let Some(q) = &quoted {
                        div { class: "line", span { "Road distance" } strong { "{q.distance_meters} m" } }
                        div { class: "line", span { "Delivery fee" } strong { "{peso(q.delivery_fee_centavos)}" } }
                        div { class: "line totals", span { "Estimated total" } strong { "{peso(subtotal + q.delivery_fee_centavos)}" } }
                    }
                } else {
                    p { "The last order submission may still have gone through. Retry the same request to confirm its result." }
                }
                if !(error)().is_empty() { p { role: "alert", "{error}" } }
                button { class: "primary", disabled: busy() || (pending.is_none() && !can_checkout),
                    onclick: move |_| {
                        if busy() { return; }
                        let request = if let Some(existing) = (state.pending)() {
                            existing
                        } else {
                            let Some(q) = (quote)().filter(|q| q.expires_at > now()) else {
                                error.set("The quote expired. Get an updated quote.".into());
                                return;
                            };
                            Checkout {
                                quote_id: q.id,
                                idempotency_key: uuid::Uuid::new_v4().to_string(),
                                items: lines.iter().map(|line| CheckoutItem {
                                    product_id: line.product_id, quantity_milli: line.quantity_milli,
                                    expected_price_centavos: line.price_centavos,
                                }).collect(),
                            }
                        };
                        state.pending.set(Some(request.clone()));
                        busy.set(true);
                        error.set(String::new());
                        let api = api.clone();
                        spawn(async move {
                            match api.send::<CheckoutResult, _>(Method::POST, "/orders", &request).await {
                                Ok(result) => {
                                    state.pending.set(None);
                                    state.basket.write().clear();
                                    state.active_order.set(Some(result.id));
                                    state.notice.set("Order placed. Submit your GCash reference from the order.".into());
                                    state.page.set(Page::Orders);
                                }
                                Err(failure) => {
                                    if failure.status.is_some_and(|status| (400..500).contains(&status) && status != 401 && status != 403) {
                                        state.pending.set(None);
                                        quote.set(None);
                                    }
                                    error.set(failure.to_string());
                                }
                            }
                            busy.set(false);
                        });
                    },
                    if busy() { "Please wait…" } else if pending.is_some() { "Confirm last submission" } else { "Place order" }
                }
            }
        }
    }
}

#[component]
fn BasketRow(line: BasketLine, editable: bool) -> Element {
    let mut state = use_context::<State>();
    let catalog = state.catalog.read();
    let product = catalog.iter().find(|p| p.id == line.product_id);
    let name = product.map_or("Unavailable product", |p| &p.name);
    let unit = product.map_or("", |p| &p.unit);
    let new_price = product
        .filter(|p| p.available && p.price_centavos != line.price_centavos)
        .map(|p| p.price_centavos);
    let id = line.product_id;
    let amount = cost(line.price_centavos, line.quantity_milli).unwrap_or_default();
    rsx! {
        div { class: "basket-row",
            div { class: "line", strong { "{name}" } strong { "{peso(amount)}" } }
            div { class: "line",
                span { "{line.quantity_milli / 1000} {unit} · {peso(line.price_centavos)} each" }
                div { class: "actions",
                    button { disabled: !editable || line.quantity_milli <= 1000, aria_label: "Decrease {name} quantity", onclick: move |_| quantity(&mut state, id, -1000), "−" }
                    button { disabled: !editable || line.quantity_milli >= MAX_QUANTITY_MILLI, aria_label: "Increase {name} quantity", onclick: move |_| quantity(&mut state, id, 1000), "+" }
                    button { disabled: !editable, class: "quiet", onclick: move |_| state.basket.write().retain(|v| v.product_id != id), "Remove" }
                }
            }
            if let Some(new_price) = new_price {
                button { disabled: !editable,
                    onclick: move |_| {
                        if let Some(v) = state.basket.write().iter_mut().find(|v| v.product_id == id) {
                            v.price_centavos = new_price;
                        }
                    },
                    "Accept new price: {peso(new_price)}"
                }
            } else if product.is_none_or(|p| !p.available) {
                p { class: "notice", "This item is unavailable. Remove it to check out." }
            }
        }
    }
}

fn quantity(state: &mut State, id: i64, delta: i64) {
    if let Some(item) = state.basket.write().iter_mut().find(|v| v.product_id == id) {
        item.quantity_milli = (item.quantity_milli + delta).clamp(1000, MAX_QUANTITY_MILLI);
    }
}
