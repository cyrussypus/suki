use crate::{
    Api, State,
    api::{Live, delay},
    now,
};
use dioxus::prelude::*;
use futures_util::{FutureExt, StreamExt};
use reqwest::Method;
use suki_shared::{
    Acknowledged,
    auth::Role,
    orders::{Decision, Order, OrderDetail, Status},
    payments::{Evidence, EvidenceResult},
    pricing::peso,
    realtime::{
        ChatMessage, CourierLocation, LocationSnapshot, MessageInput, OrderEvent, Position,
        PositionResult,
    },
};

#[component]
pub fn OrderList() -> Element {
    let api = use_context::<Api>();
    let mut state = use_context::<State>();
    let mut orders = use_signal(Vec::<Order>::new);
    let mut error = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let signed_in = state.account.read().is_some();
    let list_api = api.clone();
    let refresh = move || {
        let api = list_api.clone();
        spawn(async move {
            loading.set(true);
            match api.get("/orders").await {
                Ok(items) => {
                    orders.set(items);
                    error.set(String::new());
                }
                Err(failure) => error.set(failure.to_string()),
            }
            loading.set(false);
        });
    };
    let first = refresh.clone();
    use_effect(move || {
        if state.account.read().is_some() {
            first();
        } else {
            loading.set(false);
        }
    });
    rsx! {
        section { class: "panel",
            div { class: "line", h1 { "Orders" }
                if signed_in { button { disabled: loading(), onclick: move |_| refresh(), "Refresh" } }
            }
            if !signed_in {
                p { "Sign in to see your orders." }
                button { class: "primary", onclick: move |_| state.page.set(crate::Page::Account), "Sign in" }
            } else if loading() {
                p { "Loading orders…" }
            } else if !(error)().is_empty() {
                p { role: "alert", "{error}" }
            } else if orders.read().is_empty() {
                p { "No orders yet." }
            } else {
                for order in orders.read().iter().cloned() {
                    button { class: "order-link line", key: "{order.id}",
                        onclick: move |_| state.active_order.set(Some(order.id.clone())),
                        span { strong { "Order {short(&order.id)}" } small { "{label(&order.status)}" } }
                        strong { "{peso(order.estimated_total_centavos)}" }
                    }
                }
            }
        }
    }
}

#[component]
pub fn OrderScreen(id: String) -> Element {
    let api = use_context::<Api>();
    let mut state = use_context::<State>();
    let detail = use_signal(|| None::<OrderDetail>);
    let mut messages = use_signal(Vec::<ChatMessage>::new);
    let mut location = use_signal(|| None::<CourierLocation>);
    let mut connection = use_signal(|| "Connecting…");
    let problem = use_signal(String::new);
    let path = format!("/orders/{id}");
    let stream_path = path.clone();
    let stream_api = api.clone();
    use_future(move || {
        let api = stream_api.clone();
        let path = stream_path.clone();
        let id = id.clone();
        async move {
            let mut cursor = 0;
            let mut backoff = 1000;
            loop {
                connection.set("Connecting…");
                let mut events = api.events(&id);
                match snapshot(&api, &path, detail, location, problem).await {
                    Access::Denied => {
                        messages.write().clear();
                        connection.set("Session unavailable");
                        return;
                    }
                    Access::Retry | Access::Ready => {}
                }
                catch_up(&api, &path, &mut cursor, messages, problem).await;
                let mut tick = Box::pin(delay(10_000).fuse());
                loop {
                    let next = events.next().fuse();
                    futures_util::pin_mut!(next);
                    let event = futures_util::select! {
                        item = next => item,
                        () = tick => Some(Live::Event(OrderEvent::Resync)),
                    };
                    let Some(event) = event else {
                        break;
                    };
                    match event {
                        Live::Connected => {
                            connection.set("Live");
                            backoff = 1000;
                            catch_up(&api, &path, &mut cursor, messages, problem).await;
                        }
                        Live::Event(event) => {
                            if !matches!(event, OrderEvent::Resync) {
                                connection.set("Live");
                                backoff = 1000;
                            }
                            match event {
                                OrderEvent::Message { .. } => {
                                    catch_up(&api, &path, &mut cursor, messages, problem).await
                                }
                                OrderEvent::Location(position) => {
                                    if detail
                                        .peek()
                                        .as_ref()
                                        .is_some_and(|d| d.order.status == "out_for_delivery")
                                    {
                                        location.set(Some(position));
                                    } else {
                                        snapshot(&api, &path, detail, location, problem).await;
                                    }
                                }
                                OrderEvent::Status { .. } | OrderEvent::Resync => {
                                    if let Access::Denied =
                                        snapshot(&api, &path, detail, location, problem).await
                                    {
                                        messages.write().clear();
                                        connection.set("Session unavailable");
                                        return;
                                    }
                                    catch_up(&api, &path, &mut cursor, messages, problem).await;
                                    tick = Box::pin(delay(10_000).fuse());
                                }
                            }
                        }
                    }
                }
                connection.set("Reconnecting…");
                delay(backoff).await;
                backoff = (backoff * 2).min(15_000);
            }
        }
    });
    let data = (detail)();
    let position = (location)();
    let user = (state.account)();
    rsx! {
        button { class: "quiet", onclick: move |_| state.active_order.set(None), "← All orders" }
        if !(problem)().is_empty() { p { class: "notice", role: "alert", "{problem}" } }
        if let Some(data) = data {
            section { class: "panel",
                p { class: "eyebrow", "MARKET ORDER" }
                h1 { "Order {short(&data.order.id)}" }
                strong { class: "badge", "{label(&data.order.status)}" }
                p { "{data.order.address}" }
                for item in data.items.iter() {
                    div { class: "line", key: "{item.id}",
                        span { "{item.name} × {quantity(item.quantity_milli)} {item.unit}" }
                        strong { "{peso(item.estimated_cost_centavos)}" }
                    }
                }
                div { class: "line", span { "Estimated goods" } strong { "{peso(data.order.goods_subtotal_centavos)}" } }
                div { class: "line", span { "Delivery" } strong { "{peso(data.order.delivery_fee_centavos)}" } }
                div { class: "line totals", span { "Estimated total" } strong { "{peso(data.order.estimated_total_centavos)}" } }
                if let Some(final_goods) = data.order.final_goods_centavos {
                    p { "Final goods: {peso(final_goods)}" }
                }
            }
            Payment { data: data.clone(), path: path.clone(), detail }
            if user.as_ref().is_some_and(|u| u.has_role(Role::Customer))
                && matches!(data.order.status.as_str(), "awaiting_payment" | "payment_verification" | "paid")
                && !data.payments.iter().any(|p| p.status == "under_verification")
            {
                CancelOrder { path: path.clone(), detail }
            }
            if user.as_ref().is_some_and(|u| u.has_role(Role::Customer)) {
                for row in data.adjustments.iter().filter(|row| row.3 == "pending") {
                    Approval { key: "{row.0}", detail, path: path.clone(),
                        endpoint: format!("{path}/adjustments/{}", row.0),
                        title: format!("Spending approval: {}", peso(row.1)), description: row.2.clone() }
                }
                for row in data.substitutions.iter().filter(|row| row.5 == "pending") {
                    Approval { key: "{row.0}", detail, path: path.clone(),
                        endpoint: format!("{path}/substitutions/{}", row.0),
                        title: format!("Substitute: {}", row.2),
                        description: format!("{} at {} per unit", quantity(row.3), peso(row.4)) }
                }
            }
            section { class: "panel",
                div { class: "line", h2 { "Delivery timeline" } small { "{connection}" } }
                ol { class: "timeline",
                    for (index, (status, _, timestamp)) in data.history.iter().enumerate() {
                        li { key: "{index}", strong { "{label(status)}" } " · {date(*timestamp)}" }
                    }
                }
                if data.order.status == "out_for_delivery" {
                    if let Some(position) = position {
                        p { "Courier location: {position.lat:.5}, {position.lng:.5}" }
                        p { class: "muted", "Reported {date(position.reported_at)}"
                            if now() - position.reported_at > 15 { " · Last report is stale" }
                        }
                    } else {
                        p { "Waiting for a courier location report." }
                    }
                    if user.as_ref().is_some_and(|u| data.order.courier_id.as_deref() == Some(&u.id) && (u.has_role(Role::Courier) || u.has_role(Role::Admin))) {
                        CourierReport { path: path.clone() }
                    }
                }
            }
            Chat { path, messages }
        } else if (problem)().is_empty() {
            p { "Loading order…" }
        }
    }
}

enum Access {
    Ready,
    Retry,
    Denied,
}

async fn snapshot(
    api: &Api,
    path: &str,
    mut detail: Signal<Option<OrderDetail>>,
    mut location: Signal<Option<CourierLocation>>,
    mut problem: Signal<String>,
) -> Access {
    match api.get::<OrderDetail>(path).await {
        Ok(data) => {
            let active = data.order.status == "out_for_delivery";
            detail.set(Some(data));
            problem.set(String::new());
            if active {
                match api.get(&format!("{path}/location")).await {
                    Ok(LocationSnapshot::Report { position, .. }) => {
                        location.set(Some(position));
                    }
                    Ok(LocationSnapshot::Missing { .. }) => location.set(None),
                    Err(failure) if matches!(failure.status, Some(401 | 403 | 404)) => {
                        detail.set(None);
                        location.set(None);
                        problem.set(failure.to_string());
                        return Access::Denied;
                    }
                    Err(_) => location.set(None),
                }
            } else {
                location.set(None);
            }
            Access::Ready
        }
        Err(failure) => {
            problem.set(failure.to_string());
            if matches!(failure.status, Some(401 | 403 | 404)) {
                detail.set(None);
                location.set(None);
                Access::Denied
            } else {
                Access::Retry
            }
        }
    }
}

async fn catch_up(
    api: &Api,
    path: &str,
    cursor: &mut i64,
    mut messages: Signal<Vec<ChatMessage>>,
    mut problem: Signal<String>,
) {
    loop {
        match api
            .get::<Vec<ChatMessage>>(&format!("{path}/messages?after={cursor}"))
            .await
        {
            Ok(page) => {
                let count = page.len();
                if let Some(last) = page.last() {
                    *cursor = last.id;
                    messages.write().extend(page);
                }
                if count < 200 {
                    break;
                }
            }
            Err(failure) => {
                if matches!(failure.status, Some(401 | 403 | 404)) {
                    messages.write().clear();
                }
                problem.set(failure.to_string());
                break;
            }
        }
    }
}

#[component]
fn Chat(path: String, messages: Signal<Vec<ChatMessage>>) -> Element {
    let api = use_context::<Api>();
    let mut text = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    rsx! {
        section { class: "panel",
            h2 { "Order chat" }
            div { class: "chat", role: "log",
                if messages.read().is_empty() { p { "No messages yet." } }
                for message in messages.read().iter() {
                    article { key: "{message.id}",
                        strong { "{message.sender_name}" } small { "{date(message.created_at)}" }
                        p { "{message.content}" }
                    }
                }
            }
            form { onsubmit: move |event| {
                event.prevent_default();
                if busy() || text().trim().is_empty() { return; }
                busy.set(true);
                error.set(String::new());
                let body = MessageInput { content: text() };
                let api = api.clone();
                let endpoint = format!("{path}/messages");
                spawn(async move {
                    match api.send::<OrderEvent, _>(Method::POST, &endpoint, &body).await {
                        Ok(_) => text.set(String::new()),
                        Err(failure) => error.set(failure.to_string()),
                    }
                    busy.set(false);
                });
            },
                label { "Message" textarea { disabled: busy(), maxlength: "2000", value: "{text}", oninput: move |e| text.set(e.value()) } }
                button { class: "primary", disabled: busy() || text().trim().is_empty(), "Send message" }
            }
            if !(error)().is_empty() { p { role: "alert", "{error}" } }
        }
    }
}

#[component]
fn Payment(data: OrderDetail, path: String, mut detail: Signal<Option<OrderDetail>>) -> Element {
    let api = use_context::<Api>();
    let state = use_context::<State>();
    let mut reference = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    let paid: i64 = data
        .payments
        .iter()
        .filter(|p| p.status == "paid")
        .filter_map(|p| p.received_amount_centavos)
        .sum();
    let due = data.order.authorized_goods_centavos + data.order.delivery_fee_centavos - paid;
    let can_pay = due > 0
        && state
            .account
            .read()
            .as_ref()
            .is_some_and(|u| u.has_role(Role::Customer))
        && matches!(
            data.order.status.as_str(),
            "awaiting_payment" | "payment_verification" | "price_adjustment"
        )
        && !data
            .payments
            .iter()
            .any(|p| p.status == "under_verification");
    let qr = state
        .settings
        .read()
        .as_ref()
        .and_then(|settings| api.image(&settings.gcash_qr_url));
    rsx! {
        section { class: "panel",
            h2 { "GCash payment" }
            p { "Payment status: {label(&data.order.payment_status)}" }
            for payment in data.payments.iter() {
                div { class: "line", key: "{payment.id}", span { "{payment.reference} · {label(&payment.status)}" } strong { "{peso(payment.claimed_amount_centavos)}" } }
            }
            for (id, amount, status) in data.refunds.iter() {
                div { class: "line", key: "{id}", span { "Refund · {label(status)}" } strong { "{peso(*amount)}" } }
            }
            if can_pay {
                if let Some(qr) = qr {
                    img { class: "qr", src: "{qr}", alt: "Operator GCash payment QR" }
                }
                p { "Outstanding: {peso(due)}. An administrator verifies the payment after you submit the reference." }
                form {
                    onsubmit: move |event| {
                        event.prevent_default();
                        if busy() { return; }
                        busy.set(true);
                        error.set(String::new());
                        let api = api.clone();
                        let path = path.clone();
                        let evidence = Evidence { reference: reference(), amount_centavos: due };
                        spawn(async move {
                            match api.send::<EvidenceResult, _>(Method::POST, &format!("{path}/payments"), &evidence).await {
                                Ok(_) => {
                                    reference.set(String::new());
                                    match api.get(&path).await {
                                        Ok(data) => detail.set(Some(data)),
                                        Err(failure) => error.set(failure.to_string()),
                                    }
                                }
                                Err(failure) => error.set(failure.to_string()),
                            }
                            busy.set(false);
                        });
                    },
                    label { "GCash reference" input { required: true, disabled: busy(), minlength: "6", maxlength: "100", value: "{reference}", oninput: move |e| reference.set(e.value()) } }
                    button { class: "primary", disabled: busy(), "Submit GCash reference" }
                }
            }
            if !(error)().is_empty() { p { role: "alert", "{error}" } }
        }
    }
}

#[component]
fn CancelOrder(path: String, mut detail: Signal<Option<OrderDetail>>) -> Element {
    let api = use_context::<Api>();
    let mut confirm = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    rsx! {
        section { class: "panel",
            if confirm() {
                p { "Cancel this order? If a payment was verified, the operator will record a refund." }
                div { class: "actions",
                    button { disabled: busy(), onclick: move |_| {
                        if busy() { return; }
                        busy.set(true);
                        let api = api.clone();
                        let path = path.clone();
                        spawn(async move {
                            let result = async {
                                api.send::<Acknowledged, _>(
                                    Method::PATCH, &format!("{path}/status"), &Status { status: "canceled".into() },
                                ).await?;
                                let updated = api.get(&path).await?;
                                detail.set(Some(updated));
                                Ok::<_, crate::api::Failure>(())
                            }.await;
                            if let Err(failure) = result { error.set(failure.to_string()); }
                            busy.set(false);
                        });
                    }, "Confirm cancellation" }
                    button { disabled: busy(), onclick: move |_| confirm.set(false), "Keep order" }
                }
            } else {
                button { onclick: move |_| confirm.set(true), "Cancel order" }
            }
            if !(error)().is_empty() { p { role: "alert", "{error}" } }
        }
    }
}

#[component]
fn Approval(
    mut detail: Signal<Option<OrderDetail>>,
    path: String,
    endpoint: String,
    title: String,
    description: String,
) -> Element {
    let api = use_context::<Api>();
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    let mut decide = move |approved| {
        if busy() {
            return;
        }
        busy.set(true);
        let (api, path, endpoint) = (api.clone(), path.clone(), endpoint.clone());
        spawn(async move {
            match api
                .send::<Acknowledged, _>(Method::POST, &endpoint, &Decision { approved })
                .await
            {
                Ok(_) => match api.get(&path).await {
                    Ok(data) => detail.set(Some(data)),
                    Err(failure) => error.set(failure.to_string()),
                },
                Err(failure) => error.set(failure.to_string()),
            }
            busy.set(false);
        });
    };
    let mut decline = decide.clone();
    rsx! {
        section { class: "panel", h2 { "{title}" } p { "{description}" }
            div { class: "actions",
                button { class: "primary", disabled: busy(), onclick: move |_| decide(true), "Approve" }
                button { disabled: busy(), onclick: move |_| decline(false), "Decline" }
            }
            if !(error)().is_empty() { p { role: "alert", "{error}" } }
        }
    }
}

#[component]
fn CourierReport(path: String) -> Element {
    let api = use_context::<Api>();
    let mut lat = use_signal(String::new);
    let mut lng = use_signal(String::new);
    let mut status = use_signal(String::new);
    let mut busy = use_signal(|| false);
    rsx! {
        form { onsubmit: move |event| {
            event.prevent_default();
            if busy() { return; }
            let (Ok(lat), Ok(lng)) = (lat().parse::<f64>(), lng().parse::<f64>()) else {
                status.set("Enter valid coordinates.".into());
                return;
            };
            if !lat.is_finite() || !lng.is_finite() {
                status.set("Enter valid coordinates.".into());
                return;
            }
            let api = api.clone();
            let endpoint = format!("{path}/location");
            busy.set(true);
            spawn(async move {
                let response = api.send::<PositionResult, _>(Method::PUT, &endpoint, &Position { lat, lng }).await;
                status.set(response.map_or_else(|failure| failure.to_string(), |_| "Location reported.".into()));
                busy.set(false);
            });
        },
            div { class: "field-pair",
                label { "Courier latitude" input { required: true, inputmode: "decimal", value: "{lat}", oninput: move |e| lat.set(e.value()) } }
                label { "Courier longitude" input { required: true, inputmode: "decimal", value: "{lng}", oninput: move |e| lng.set(e.value()) } }
            }
            button { disabled: busy(), "Report location" }
            p { role: "status", "{status}" }
        }
    }
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

fn label(status: &str) -> String {
    status.replace('_', " ")
}

fn quantity(milli: i64) -> String {
    if milli % 1000 == 0 {
        (milli / 1000).to_string()
    } else {
        format!("{}.{:03}", milli / 1000, milli % 1000)
            .trim_end_matches('0')
            .into()
    }
}

fn date(timestamp: i64) -> String {
    // Davao observes UTC+8 year-round.
    time::OffsetDateTime::from_unix_timestamp(timestamp)
        .map(|utc| {
            let local = utc.to_offset(time::macros::offset!(+8));
            local
                .format(time::macros::format_description!(
                    "[month repr:short] [day], [hour]:[minute]"
                ))
                .unwrap_or_default()
        })
        .unwrap_or_default()
}
