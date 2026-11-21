use crate::{Api, State};
use dioxus::prelude::*;
use reqwest::Method;
use serde::Serialize;
use suki_shared::{
    Acknowledged, StatusResult,
    admin::{AuditEntry, Reports, UserUpdate},
    auth::{Account, Role},
    orders::{Assignment, Item, OrderDetail, Purchase, Reconciliation, Status},
    payments::{CashCollection, Payment, RefundReference, Verification},
    pricing::peso,
};

#[component]
pub fn AdminDashboard() -> Element {
    let api = use_context::<Api>();
    let mut dashboard = use_resource(move || {
        let api = api.clone();
        async move {
            let (pending, accounts, reports, audit) = futures_util::join!(
                api.get::<Vec<Payment>>("/admin/payments/pending"),
                api.get::<Vec<Account>>("/admin/users"),
                api.get::<Reports>("/admin/reports"),
                api.get::<Vec<AuditEntry>>("/admin/audit"),
            );
            Ok::<_, crate::api::Failure>((pending?, accounts?, reports?, audit?))
        }
    });
    rsx! {
        div { class: "line", h1 { "Admin dashboard" } button { onclick: move |_| dashboard.restart(), "Refresh dashboard" } }
        match &*dashboard.read() {
            Some(Ok((pending, accounts, reports, audit))) => rsx! {
                section { class: "panel", h2 { "GCash reference audit" }
                    if pending.is_empty() { p { "No pending references." } }
                    for payment in pending { VerifyPayment { key: "{payment.id}", payment: payment.clone(), onsaved: move |_| dashboard.restart() } }
                }
                section { class: "panel", h2 { "Transaction history" }
                    p { "View an order to assign a rider or record a processed refund." }
                    for order in reports.orders.iter().take(30) {
                        OrderLink { key: "{order.order_id}", id: order.order_id.clone(), caption: format!("{} collected · {} refunded", peso(order.received_centavos), peso(order.refunds_centavos)) }
                    }
                }
                details { class: "panel", summary { "Role management" }
                    for account in accounts {
                        ManageUser { key: "{account.id}", account: account.clone(), onsaved: move |_| dashboard.restart() }
                    }
                }
                details { class: "panel", summary { "Recent audit activity" }
                    for event in audit.iter().take(30) {
                        p { key: "{event.id}", "{event.action} · {event.entity_id}" }
                    }
                }
            },
            Some(Err(error)) => rsx! { p { role: "alert", "{error}" } },
            None => rsx! { p { "Loading dashboard…" } },
        }
    }
}

#[component]
fn OrderLink(id: String, caption: String) -> Element {
    let mut state = use_context::<State>();
    rsx! {
        p { button { class: "order-link", onclick: move |_| {
            state.active_order.set(Some(id.clone()));
            state.page.set(crate::Page::Orders);
        }, "Order {id.chars().take(8).collect::<String>()} · {caption}" } }
    }
}

#[component]
fn ManageUser(account: Account, onsaved: EventHandler<()>) -> Element {
    let state = use_context::<State>();
    let mut role = use_signal(|| account.role.clone());
    let mut status = use_signal(|| account.status.clone());
    let id = account.id.clone();
    let api = use_context::<Api>();
    let mut busy = use_signal(|| false);
    let mut message = use_signal(String::new);
    let self_user = state.account.read().as_ref().is_some_and(|u| u.id == id);
    rsx! {
        form { class: "order-link", onsubmit: move |e| {
            e.prevent_default();
            if self_user || busy() { return; }
            let path = format!("/admin/users/{id}");
            let body = UserUpdate { role: role(), status: status() };
            let api = api.clone();
            busy.set(true);
            spawn(async move {
                match api.send::<Acknowledged, _>(Method::PATCH, &path, &body).await {
                    Ok(_) => { message.set("Account updated.".into()); onsaved.call(()); }
                    Err(e) => message.set(e.to_string()),
                }
                busy.set(false);
            });
        },
            strong { "{account.name} · {account.email}" }
            if !self_user {
                label { "Role" select { value: "{role}", onchange: move |e| role.set(e.value()),
                    option { value: "customer", "Customer" }
                    option { value: "vendor", "Vendor" }
                    option { value: "courier", "Rider" }
                    option { value: "admin", "Admin" }
                } }
                label { "Status" select { value: "{status}", onchange: move |e| status.set(e.value()),
                    for value in ["pending", "active", "suspended", "deactivated"] {
                        option { value: "{value}", "{value}" }
                    }
                } }
                button { disabled: busy(), "Save account" }
                p { role: "status", "{message}" }
            }
        }
    }
}

#[component]
fn VerifyPayment(payment: Payment, onsaved: EventHandler<()>) -> Element {
    let api = use_context::<Api>();
    let mut busy = use_signal(|| false);
    let mut received = use_signal(String::new);
    let mut error = use_signal(String::new);
    let order = payment.order_id.clone();
    let reference = payment.reference.clone();
    let amount = payment.claimed_amount_centavos;
    let mut submit = move |approved: bool| {
        if busy() {
            return;
        }
        let expected = if approved {
            received().parse::<i64>().ok()
        } else {
            Some(0)
        };
        let Some(received_amount_centavos) = expected else {
            error.set("Enter the actual amount received in centavos.".into());
            return;
        };
        let endpoint = format!("/payments/{}/verify", payment.id);
        let api = api.clone();
        busy.set(true);
        spawn(async move {
            match api
                .send::<StatusResult, _>(
                    Method::POST,
                    &endpoint,
                    &Verification {
                        approved,
                        received_amount_centavos,
                    },
                )
                .await
            {
                Ok(_) => {
                    error.set(String::new());
                    onsaved.call(());
                }
                Err(e) => error.set(e.to_string()),
            };
            busy.set(false);
        });
    };
    let mut reject = submit.clone();
    rsx! {
        article { class: "order-link",
            OrderLink { id: order, caption: format!("{reference} · {}", peso(amount)) }
            label { "Amount received (centavos)" input { r#type: "number", min: "0", value: "{received}", oninput: move |e| received.set(e.value()) } }
            div { class: "actions",
                button { disabled: busy(), onclick: move |_| submit(true), "Verify payment" }
                button { disabled: busy(), onclick: move |_| reject(false), "Reject unpaid reference" }
            }
            if !error().is_empty() { p { role: "alert", "{error}" } }
        }
    }
}

#[component]
pub fn StaffOrder(
    data: OrderDetail,
    account: Account,
    mut detail: Signal<Option<OrderDetail>>,
) -> Element {
    let api = use_context::<Api>();
    let path = format!("/orders/{}", data.order.id);
    let mut message = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut reference = use_signal(String::new);
    let mut received = use_signal(String::new);
    let mut rider_id = use_signal(String::new);
    let admin = account.has_role(Role::Admin);
    let assigned = data.order.courier_id.as_deref() == Some(&account.id);
    let refresh_api = api.clone();
    let refresh_path = path.clone();
    let refresh = move |_| {
        let api = refresh_api.clone();
        let path = refresh_path.clone();
        spawn(async move {
            match api.get(&path).await {
                Ok(updated) => detail.set(Some(updated)),
                Err(error) => message.set(error.to_string()),
            }
        });
    };
    let mutate_api = api.clone();
    let mutate_path = path.clone();
    let mutate = move |(method, endpoint, body): (Method, String, serde_json::Value)| {
        if busy() {
            return;
        }
        let api = mutate_api.clone();
        let root = mutate_path.clone();
        busy.set(true);
        spawn(async move {
            let result = match api
                .send::<serde_json::Value, _>(method, &endpoint, &body)
                .await
            {
                Ok(_) => api.get(&root).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(updated) => {
                    detail.set(Some(updated));
                    message.set("Order updated.".into());
                }
                Err(error) => message.set(error.to_string()),
            }
            busy.set(false);
        });
    };
    let next = match data.order.status.as_str() {
        "paid" | "confirmed" => Some(("shopping", "Start market pickup")),
        "ready_for_delivery" if data.order.courier_id.is_some() => {
            Some(("out_for_delivery", "Start delivery"))
        }
        "out_for_delivery" if data.order.payment_status != "cod_pending" => {
            Some(("delivered", "Mark delivered"))
        }
        "delivered" if data.order.vendor_id.is_none() || data.settlement.is_some() => {
            Some(("completed", "Complete order"))
        }
        _ => None,
    };
    let mut change_status = mutate.clone();
    let mut assign = mutate.clone();
    let mut collect = mutate.clone();
    let mut settle = mutate.clone();
    let status_path = format!("{path}/status");
    let assignment_path = format!("{path}/assign");
    let collect_path = format!("{path}/cash-collection");
    let settle_path = format!("{path}/vendor-settlement");
    let reconcile_refresh = refresh.clone();
    let reconcile_path = format!("{path}/reconcile");
    rsx! {
        section { class: "panel",
            h2 { if admin { "Order operations" } else { "Rider pickup and payment" } }
            if admin && !matches!(data.order.status.as_str(), "out_for_delivery" | "delivered" | "completed" | "canceled") {
                label { "Rider account ID" input { value: "{rider_id}", oninput: move |e| rider_id.set(e.value()) } }
                button { disabled: busy() || rider_id().trim().is_empty(), onclick: move |_| assign((
                    Method::PUT, assignment_path.clone(), body(&Assignment { courier_id: rider_id().trim().into() })
                )), "Assign rider" }
                RiderDirectory {}
            }
            if assigned && data.order.vendor_id.is_some() && !data.order.is_cod() {
                if let Some(number) = &data.order.gcash_number {
                    p { "Verify the transfer against the vendor's GCash number: " strong { "{number}" } }
                }
                for payment in data.payments.iter().filter(|p| p.status == "under_verification") {
                    VerifyPayment { key: "{payment.id}", payment: payment.clone(), onsaved: refresh.clone() }
                }
            }
            if assigned || admin {
                if let Some((status, title)) = next {
                    button { disabled: busy(), onclick: move |_| change_status((
                        Method::PATCH, status_path.clone(), body(&Status { status: status.into() })
                    )), "{title}" }
                }
                if data.order.status == "shopping" {
                    for item in &data.items {
                        PurchaseRow { key: "{item.id}", item: item.clone(), path: path.clone(), onsaved: refresh.clone() }
                    }
                    button { disabled: busy() || data.items.iter().any(|i| i.actual_cost_centavos.is_none()), onclick: move |_| {
                        if busy() { return; }
                        let api = api.clone();
                        let path = reconcile_path.clone();
                        let refresh = reconcile_refresh.clone();
                        busy.set(true);
                        spawn(async move {
                            match api.send::<Reconciliation, _>(Method::POST, &path, &()).await {
                                Ok(_) => refresh(()),
                                Err(e) => message.set(e.to_string()),
                            }
                            busy.set(false);
                        });
                    }, "Reconcile purchases" }
                }
            }
            if assigned && data.order.is_cod() && data.order.payment_status == "cod_pending" && data.order.status == "out_for_delivery" {
                p { "Cash due at handoff: {peso(data.order.final_goods_centavos.unwrap_or_default() + data.order.delivery_fee_centavos)}." }
                label { "Cash received (centavos)" input { r#type: "number", min: "0", value: "{received}", oninput: move |e| received.set(e.value()) } }
                button { disabled: busy() || received().parse::<i64>().is_err(), onclick: move |_| {
                    if let Ok(amount_centavos) = received().parse() {
                        collect((Method::POST, collect_path.clone(), body(&CashCollection { amount_centavos })));
                    }
                }, "Record cash collected" }
            }
            if let Some(settlement) = &data.settlement {
                p { "Vendor settlement recorded: {peso(settlement.amount_centavos)} · {settlement.reference}" }
            } else if assigned && data.order.vendor_id.is_some() && data.order.status == "delivered" {
                p { if data.order.is_cod() {
                    "Transfer the reconciled goods proceeds to the vendor and record the receipt."
                } else {
                    "Confirm the vendor received the direct transfer. Do not send a second payment."
                } }
                label { "Vendor settlement reference" input { value: "{reference}", oninput: move |e| reference.set(e.value()) } }
                button { disabled: busy() || reference().trim().len() < 6, onclick: move |_| settle((
                    Method::POST, settle_path.clone(), body(&RefundReference { reference: reference() })
                )), "Record vendor settlement" }
            }
            if admin && matches!(data.order.status.as_str(), "ready_for_delivery" | "out_for_delivery" | "delivered" | "completed" | "canceled") {
                for (id, amount, status) in data.refunds.iter().filter(|r| r.2 == "pending") {
                    RefundRow { key: "{id}", id: id.clone(), amount: *amount, status: status.clone(), onsaved: refresh.clone() }
                }
            }
            p { role: "status", "{message}" }
        }
    }
}

#[component]
fn RiderDirectory() -> Element {
    let api = use_context::<Api>();
    let accounts = use_resource(move || {
        let api = api.clone();
        async move { api.get::<Vec<Account>>("/admin/users").await }
    });
    rsx! {
        details { summary { "Active rider accounts" }
            if let Some(Ok(accounts)) = &*accounts.read() {
                for account in accounts.iter().filter(|a| a.role == "courier" && a.status == "active") {
                    p { key: "{account.id}", "{account.name} · {account.id}" }
                }
            }
        }
    }
}

#[component]
fn RefundRow(id: String, amount: i64, status: String, onsaved: EventHandler<()>) -> Element {
    let api = use_context::<Api>();
    let mut reference = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    rsx! {
        form { onsubmit: move |e| {
            e.prevent_default();
            if busy() { return; }
            let api = api.clone();
            let path = format!("/refunds/{id}/process");
            let body = RefundReference { reference: reference() };
            busy.set(true);
            spawn(async move {
                match api.send::<Acknowledged, _>(Method::POST, &path, &body).await {
                    Ok(_) => onsaved.call(()),
                    Err(e) => error.set(e.to_string()),
                }
                busy.set(false);
            });
        },
            p { "Refund {peso(amount)} · {status}" }
            label { "Receipt for refund already sent" input { required: true, minlength: "6", maxlength: "100", value: "{reference}", oninput: move |e| reference.set(e.value()) } }
            button { disabled: busy(), "Record refund" }
            if !error().is_empty() { p { role: "alert", "{error}" } }
        }
    }
}

#[component]
fn PurchaseRow(item: Item, path: String, onsaved: EventHandler<()>) -> Element {
    let api = use_context::<Api>();
    let mut quantity = use_signal(|| item.quantity_milli.to_string());
    let mut price = use_signal(|| item.unit_price_centavos.to_string());
    let mut unavailable = use_signal(|| item.unavailable);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    rsx! {
        form { class: "order-link", onsubmit: move |e| {
            e.prevent_default();
            if busy() { return; }
            let (Ok(actual_quantity_milli), Ok(actual_unit_price_centavos)) = (quantity().parse(), price().parse()) else {return;};
            let input = if unavailable() {
                Purchase { actual_quantity_milli: 0, actual_unit_price_centavos: 0, unavailable: true }
            } else {
                Purchase { actual_quantity_milli, actual_unit_price_centavos, unavailable: false }
            };
            let api = api.clone();
            let endpoint = format!("{path}/items/{}/purchase", item.id);
            busy.set(true);
            spawn(async move {
                match api.send::<suki_shared::orders::PurchaseResult, _>(Method::PUT, &endpoint, &input).await {
                    Ok(_) => { error.set("Purchase recorded.".into()); onsaved.call(()); }
                    Err(e) => error.set(e.to_string()),
                }
                busy.set(false);
            });
        },
            strong { "{item.name}" }
            label { "Unavailable" input { r#type: "checkbox", checked: unavailable(), onchange: move |e| unavailable.set(e.checked()) } }
            label { "Purchased quantity (thousandths)" input { r#type: "number", min: "0", required: true, value: "{quantity}", oninput: move |e| quantity.set(e.value()) } }
            label { "Unit price (centavos)" input { r#type: "number", min: "0", required: true, value: "{price}", oninput: move |e| price.set(e.value()) } }
            button { disabled: busy(), "Record purchase" }
            p { role: "status", "{error}" }
        }
    }
}

fn body(value: &impl Serialize) -> serde_json::Value {
    serde_json::to_value(value).expect("staff input is serializable")
}
