use crate::{Api, Page, State, now};
use dioxus::prelude::*;
use reqwest::Method;
use suki_shared::{
    addresses::Address,
    auth::Role,
    catalog::BasketLine,
    delivery::{
        DeliveryQuote, MAX_CART_WEIGHT_GRAMS, WeightedDestination, grams_rounded_up,
        line_weight_milligrams,
    },
    orders::{Checkout, CheckoutItem, CheckoutResult},
    payments::PaymentMethod,
    pricing::{MAX_QUANTITY_MILLI, cost, permits_goods_subtotal, peso},
};

#[component]
pub fn Basket() -> Element {
    let api = use_context::<Api>();
    let mut state = use_context::<State>();
    let mut address = use_signal(String::new);
    let mut latitude = use_signal(String::new);
    let mut longitude = use_signal(String::new);
    let mut payment = use_signal(|| PaymentMethod::CashOnDelivery);
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
    let mut vendors = std::collections::BTreeSet::new();
    let milligrams: i64 = lines
        .iter()
        .filter_map(|line| {
            let product = catalog.iter().find(|p| p.id == line.product_id)?;
            vendors.insert(&product.vendor_id);
            line_weight_milligrams(product.weight_grams, line.quantity_milli)
        })
        .sum();
    let weight_grams = grams_rounded_up(milligrams);
    let single_vendor = vendors.len() == 1;
    let valid_weight = (1..=MAX_CART_WEIGHT_GRAMS).contains(&weight_grams);
    let quoted = (quote)().filter(|q| q.weight_grams == Some(weight_grams));
    let can_checkout = minimum_met
        && !has_changes
        && single_vendor
        && valid_weight
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
            }
            if !lines.is_empty() {
                p { "Estimated weight: {weight_grams} g" }
                if !single_vendor { p { class: "notice", "Choose items from one vendor stall per order." } }
                if !valid_weight { p { class: "notice", "The basket must weigh between 1 g and 100 kg." } }
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
                            if !single_vendor || !valid_weight || has_changes { return; }
                            let destination = WeightedDestination { address: address(), lat, lng, weight_grams };
                            let api = quote_api.clone();
                            quote.set(None);
                            busy.set(true);
                            error.set(String::new());
                            spawn(async move {
                                match api.send(Method::POST, "/quotes/delivery", &destination).await {
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
                            button { r#type: "submit", disabled: !single_vendor || !valid_weight || has_changes, "Get Bankerohan delivery quote" }
                        }
                    }
                    label { "Payment method"
                        select { disabled: !allow_edit, value: payment().as_str(), onchange: move |e| {
                            payment.set(if e.value() == "gcash" { PaymentMethod::Gcash } else { PaymentMethod::CashOnDelivery });
                        },
                            option { value: "cash_on_delivery", "Cash on delivery" }
                            option { value: "gcash", "GCash transfer to vendor" }
                        }
                    }
                    p { class: "muted", "Delivery: ₱85.00 minimum · ₱7.00/km · ₱5.00/kg." }
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
                            let Some(q) = (quote)().filter(|q| q.expires_at > now() && q.weight_grams == Some(weight_grams)) else {
                                error.set("The quote expired. Get an updated quote.".into());
                                return;
                            };
                            Checkout {
                                quote_id: q.id,
                                payment_method: payment(),
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
                            match api.send::<CheckoutResult, _>(Method::POST, "/orders/checkout", &request).await {
                                Ok(result) => {
                                    state.pending.set(None);
                                    state.basket.write().clear();
                                    let valid = result.snapshot.as_ref().is_some_and(|snapshot| snapshot.valid_digest_for(&result.id));
                                    state.active_order.set(Some(result.id));
                                    state.notice.set(if result.snapshot_invalid {
                                        "Order already placed. Its checkout receipt failed verification. Contact support before paying."
                                    } else if !valid {
                                        "Order placed. Open the order to check its checkout receipt."
                                    } else { match request.payment_method {
                                        PaymentMethod::Gcash => "Order placed. Open the order to see the vendor's GCash number and submit your reference.",
                                        PaymentMethod::CashOnDelivery => "Order placed. Pay your rider when your order arrives.",
                                    }}.into());
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
