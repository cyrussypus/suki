use crate::{
    Api, State,
    api::delay,
    orders::{Chat, catch_up},
    refresh_catalog,
};
use dioxus::{html::FileData, prelude::*};
use futures_util::{FutureExt, StreamExt};
use reqwest::Method;
use suki_shared::{
    Acknowledged, Created,
    catalog::{
        CreateProductDto, MAX_PHOTO_BYTES, Product, ProductAvailability, UpdatePriceDto,
        VendorPaymentProfile,
    },
    pricing::peso,
    vendors::{Availability, VendorItem},
};

#[component]
pub fn VendorDashboard(vendor_id: String) -> Element {
    let api = use_context::<Api>();
    let state = use_context::<State>();
    let mut editing = use_signal(|| None::<Option<Product>>);
    let mut active_chat = use_signal(|| None::<String>);
    let mut queue = use_resource(move || {
        let api = api.clone();
        async move { api.get::<Vec<VendorItem>>("/vendor/items").await }
    });
    use_future(move || async move {
        loop {
            delay(15_000).await;
            queue.restart();
        }
    });
    let products: Vec<_> = state
        .catalog
        .read()
        .iter()
        .filter(|p| p.vendor_id == vendor_id)
        .cloned()
        .collect();
    rsx! {
        h1 { "Vendor dashboard" }
        PaymentProfile {}
        section { class: "panel",
            div { class: "line",
                h2 { "Your inventory" }
                button { class: "primary", onclick: move |_| editing.set(Some(None)), "Add product" }
            }
            if products.is_empty() { p { "Add your first product to open your stall." } }
            div { class: "product-grid",
                for product in products {
                    InventoryRow { key: "{product.id}", product: product.clone(), onedit: move |p| editing.set(Some(Some(p))) }
                }
            }
        }
        if let Some(product) = editing() {
            ProductForm { key: "{product.as_ref().map_or(0, |p| p.id)}", product, onclose: move |_| editing.set(None) }
        }
        section { class: "panel",
            div { class: "line", h2 { "Incoming orders" } button { onclick: move |_| queue.restart(), "Refresh" } }
            match &*queue.read() {
                Some(Ok(rows)) => rsx! {
                    if rows.is_empty() { p { "No incoming orders." } }
                    for row in rows {
                        IncomingRow {
                            key: "{row.item_id}", row: row.clone(),
                            onchat: move |id| active_chat.set(Some(id)),
                            onconfirm: move |_| queue.restart(),
                        }
                    }
                },
                Some(Err(error)) => rsx! { p { role: "alert", "{error}" } },
                None => rsx! { p { "Loading orders…" } },
            }
        }
        if let Some(id) = active_chat() {
            section { class: "panel", button { onclick: move |_| active_chat.set(None), "Close chat" }
                VendorChat { key: "{id}", id }
            }
        }
    }
}

#[component]
fn PaymentProfile() -> Element {
    let api = use_context::<Api>();
    let mut number = use_signal(String::new);
    let mut ready = use_signal(|| false);
    let mut message = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let load = api.clone();
    use_future(move || {
        let load = load.clone();
        async move {
            match load
                .get::<VendorPaymentProfile>("/vendor/payment-profile")
                .await
            {
                Ok(value) => {
                    number.set(value.gcash_number.unwrap_or_default());
                    ready.set(true);
                }
                Err(error) => message.set(error.to_string()),
            }
        }
    });
    rsx! {
        section { class: "panel",
            h2 { "Direct GCash payments" }
            p { "Customers see this number only on orders placed with GCash. Leave it empty to accept cash on delivery only." }
            form { onsubmit: move |e| {
                e.prevent_default();
                if busy() || !ready() { return; }
                let api = api.clone();
                let raw = number().trim().to_owned();
                let body = VendorPaymentProfile { gcash_number: (!raw.is_empty()).then_some(raw) };
                busy.set(true);
                spawn(async move {
                    let result = async {
                        api.confirm_sensitive_action().await?;
                        api.send::<Acknowledged, _>(Method::PUT, "/vendor/payment-profile", &body).await
                    }.await;
                    message.set(result.map_or_else(|e| e.to_string(), |_| "Payment profile saved.".into()));
                    busy.set(false);
                });
            },
                label { "GCash mobile number" input { inputmode: "tel", maxlength: "11", disabled: busy() || !ready(), value: "{number}", oninput: move |e| number.set(e.value()) } }
                button { disabled: busy() || !ready(), "Save payment profile" }
                p { role: "status", "{message}" }
            }
        }
    }
}

#[component]
fn InventoryRow(product: Product, onedit: EventHandler<Product>) -> Element {
    let api = use_context::<Api>();
    let mut state = use_context::<State>();
    let mut price = use_signal(|| product.price_centavos.to_string());
    let mut busy = use_signal(|| false);
    let pid = product.id;
    let available = product.available;
    let toggle_api = api.clone();
    let edit = product.clone();
    rsx! {
        article { class: "product-card",
            if let Some(url) = api.image(&product.image) {
                div { class: "product-photo", img { src: "{url}", alt: "{product.name}" } }
            }
            div { class: "product-info",
                h3 { "{product.name}" }
                p { "{peso(product.price_centavos)} per {product.unit} · {product.weight_grams} g" }
                form { onsubmit: move |e| {
                    e.prevent_default();
                    if busy() { return; }
                    let Ok(price_centavos) = price().parse::<i64>() else { return; };
                    let api = api.clone();
                    busy.set(true);
                    spawn(async move {
                        match api.send::<Acknowledged, _>(Method::PATCH, &format!("/vendor/goods/{pid}/price"), &UpdatePriceDto { price_centavos }).await {
                            Ok(_) => refresh_catalog(&api, state).await,
                            Err(error) => state.notice.set(error.to_string()),
                        }
                        busy.set(false);
                    });
                },
                    label { "Price (centavos)" input { r#type: "number", min: "1", required: true, value: "{price}", oninput: move |e| price.set(e.value()) } }
                    button { disabled: busy(), "Save price" }
                }
                button { disabled: busy(), onclick: move |_| {
                    if busy() { return; }
                    let api = toggle_api.clone();
                    busy.set(true);
                    spawn(async move {
                        match api.send::<Acknowledged, _>(Method::PATCH, &format!("/vendor/goods/{pid}/availability"), &ProductAvailability { available: !available }).await {
                            Ok(_) => refresh_catalog(&api, state).await,
                            Err(error) => state.notice.set(error.to_string()),
                        }
                        busy.set(false);
                    });
                }, if available { "Mark sold out" } else { "Restock" } }
                button { disabled: busy(), onclick: move |_| onedit.call(edit.clone()), "Edit product" }
            }
        }
    }
}

#[component]
fn ProductForm(product: Option<Product>, onclose: EventHandler<()>) -> Element {
    let api = use_context::<Api>();
    let state = use_context::<State>();
    let pid = product.as_ref().map(|p| p.id);
    let mut name = use_signal(|| product.as_ref().map_or(String::new(), |p| p.name.clone()));
    let mut description = use_signal(|| {
        product
            .as_ref()
            .map_or(String::new(), |p| p.description.clone())
    });
    let mut category = use_signal(|| {
        product.as_ref().map_or_else(
            || state.categories.peek().first().cloned().unwrap_or_default(),
            |p| p.category.clone(),
        )
    });
    let mut unit = use_signal(|| product.as_ref().map_or("kg".into(), |p| p.unit.clone()));
    let mut grams = use_signal(|| {
        product
            .as_ref()
            .map_or(1000, |p| p.weight_grams)
            .to_string()
    });
    let mut price = use_signal(|| {
        product
            .as_ref()
            .map_or(String::new(), |p| p.price_centavos.to_string())
    });
    let mut file = use_signal(|| None::<FileData>);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    rsx! {
        dialog { open: true, class: "panel editor",
            h2 { if pid.is_some() { "Edit product" } else { "Add product" } }
            form { onsubmit: move |e| {
                e.prevent_default();
                if busy() { return; }
                let (Ok(weight_grams), Ok(price_centavos)) = (grams().parse(), price().parse()) else {
                    error.set("Enter price and weight as whole numbers.".into());
                    return;
                };
                let selected = file();
                if pid.is_none() && selected.is_none() {
                    error.set("Select a JPEG, PNG or WebP photo.".into());
                    return;
                }
                let body = CreateProductDto { name: name(), description: description(), category: category(), unit: unit(), weight_grams, price_centavos };
                let api = api.clone();
                busy.set(true);
                error.set(String::new());
                spawn(async move {
                    let photo = if let Some(selected) = selected {
                        match selected.read_bytes().await {
                            Ok(data) if data.len() <= MAX_PHOTO_BYTES => Some(data.to_vec()),
                            _ => { error.set("Cannot read the photo or it is larger than 5 MiB.".into()); busy.set(false); return; }
                        }
                    } else { None };
                    let result = if let Some(id) = pid {
                        api.upload::<Acknowledged>(Method::PUT, &format!("/vendor/goods/{id}"), &body, photo).await.map(|_| ())
                    } else {
                        api.upload::<Created<u64>>(Method::POST, "/vendor/goods", &body, photo).await.map(|_| ())
                    };
                    match result {
                        Ok(()) => { refresh_catalog(&api, state).await; busy.set(false); onclose.call(()); }
                        Err(failure) => { error.set(failure.to_string()); busy.set(false); }
                    }
                });
            },
                label { "Product name" input { required: true, maxlength: "120", value: "{name}", oninput: move |e| name.set(e.value()) } }
                label { "Description" textarea { maxlength: "4000", value: "{description}", oninput: move |e| description.set(e.value()) } }
                label { "Category" select { required: true, value: "{category}", onchange: move |e| category.set(e.value()),
                    for option in state.categories.read().iter() { option { key: "{option}", value: "{option}", "{option}" } }
                } }
                label { "Selling unit" input { required: true, maxlength: "30", value: "{unit}", oninput: move |e| unit.set(e.value()) } }
                label { "Weight per selling unit (grams)" input { r#type: "number", required: true, min: "1", max: "50000", value: "{grams}", oninput: move |e| grams.set(e.value()) } }
                label { "Price (centavos)" input { r#type: "number", required: true, min: "1", value: "{price}", oninput: move |e| price.set(e.value()) } }
                label { "Product photo" input { r#type: "file", accept: "image/jpeg,image/png,image/webp", onchange: move |e| {
                    let selected = e.files().into_iter().next();
                    if selected.as_ref().is_some_and(|f| f.size() > MAX_PHOTO_BYTES as u64) {
                        file.set(None);
                        error.set("Photo exceeds 5 MiB.".into());
                    } else { file.set(selected); error.set(String::new()); }
                } } }
                if let Some(file) = file() { p { "Selected: {file.name()}" } }
                if !error().is_empty() { p { role: "alert", "{error}" } }
                div { class: "actions",
                    button { class: "primary", disabled: busy(), "Save product" }
                    button { r#type: "button", disabled: busy(), onclick: move |_| onclose.call(()), "Cancel" }
                }
            }
        }
    }
}

#[component]
fn IncomingRow(
    row: VendorItem,
    onchat: EventHandler<String>,
    onconfirm: EventHandler<()>,
) -> Element {
    let api = use_context::<Api>();
    let mut state = use_context::<State>();
    let mut quantity = use_signal(|| {
        row.available_quantity_milli
            .unwrap_or(row.quantity_milli)
            .to_string()
    });
    let mut busy = use_signal(|| false);
    let can_confirm = matches!(
        row.order_status.as_str(),
        "confirmed" | "paid" | "shopping" | "price_adjustment"
    );
    rsx! {
        article { class: "order-link",
            p { "{row.name} · {row.quantity_milli} thousandths of {row.unit} · {row.order_status}" }
            if let (Some(unit_price), Some(line_total)) = (row.unit_price_centavos, row.line_total_centavos) {
                p {
                    if row.snapshot_verified { "Locked at checkout: " } else { "Original order price: " }
                    "{peso(unit_price)} per {row.unit} · {peso(line_total)} for this line"
                }
            }
            if row.chat_enabled { button { onclick: move |_| onchat.call(row.order_id.clone()), "Open order chat" } }
            if can_confirm {
                form { onsubmit: move |e| {
                    e.prevent_default();
                    if busy() { return; }
                    let Ok(available_quantity_milli) = quantity().parse() else { return; };
                    let api = api.clone();
                    let path = format!("/vendor/items/{}", row.item_id);
                    busy.set(true);
                    spawn(async move {
                        match api.send::<Acknowledged, _>(Method::PUT, &path, &Availability { available_quantity_milli }).await {
                            Ok(_) => onconfirm.call(()),
                            Err(error) => state.notice.set(error.to_string()),
                        }
                        busy.set(false);
                    });
                },
                    label { "Available quantity (thousandths)" input { r#type: "number", min: "0", max: "{row.quantity_milli}", required: true, value: "{quantity}", oninput: move |e| quantity.set(e.value()) } }
                    button { disabled: busy(), "Confirm availability" }
                }
            }
        }
    }
}

#[component]
fn VendorChat(id: String) -> Element {
    let api = use_context::<Api>();
    let messages = use_signal(Vec::new);
    let error = use_signal(String::new);
    let path = format!("/orders/{id}");
    let chat_path = path.clone();
    use_future(move || {
        let (api, id, path) = (api.clone(), id.clone(), chat_path.clone());
        async move {
            let mut cursor = 0;
            loop {
                let mut events = api.events(&id);
                catch_up(&api, &path, &mut cursor, messages, error).await;
                loop {
                    let next = events.next().fuse();
                    let timeout = delay(10_000).fuse();
                    futures_util::pin_mut!(next, timeout);
                    let event = futures_util::select! { event = next => event, () = timeout => Some(crate::api::Live::Connected) };
                    if event.is_none() {
                        break;
                    }
                    catch_up(&api, &path, &mut cursor, messages, error).await;
                }
                delay(2_000).await;
            }
        }
    });
    rsx! {
        if !error().is_empty() { p { class: "notice", role: "alert", "{error}" } }
        Chat { path, messages }
    }
}
