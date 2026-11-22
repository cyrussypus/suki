use crate::{
    Api, State,
    api::{Live, delay},
    now,
};
use dioxus::prelude::*;
use futures_util::StreamExt;
use reqwest::Method;
use suki_shared::{
    Acknowledged,
    orders::Order,
    realtime::{CourierLocation, Eta, OrderEvent},
    reviews::{OrderReviews, Reputation, ReviewInput},
};

#[component]
pub fn LiveMap(order: Order, mut location: Signal<Option<CourierLocation>>) -> Element {
    let api = use_context::<Api>();
    let state = use_context::<State>();
    let mut eta = use_signal(|| None::<Eta>);
    let mut status = use_signal(|| "Connecting to rider…".to_owned());
    let oid = order.id.clone();
    let stream_api = api.clone();
    use_future(move || {
        let api = stream_api.clone();
        let oid = oid.clone();
        async move {
            loop {
                let mut stream = api.tracking_events(&oid);
                while let Some(event) = stream.next().await {
                    match event {
                        Live::Connected => status.set("Live tracking connected".into()),
                        Live::Event(OrderEvent::Location(position)) => location.set(Some(position)),
                        _ => {}
                    }
                }
                status.set("Reconnecting to rider…".into());
                delay(3_000).await;
            }
        }
    });
    let oid = order.id.clone();
    use_future(move || {
        let api = api.clone();
        let oid = oid.clone();
        async move {
            loop {
                if location
                    .peek()
                    .as_ref()
                    .is_some_and(|p| now() - p.reported_at <= 30)
                {
                    eta.set(api.get::<Eta>(&format!("/orders/{oid}/eta")).await.ok());
                } else {
                    eta.set(None);
                    delay(2_000).await;
                    continue;
                }
                delay(35_000).await;
            }
        }
    });
    let pickup = if order.weight_grams.is_some() {
        (
            suki_shared::delivery::BANKEROHAN_LAT,
            suki_shared::delivery::BANKEROHAN_LNG,
        )
    } else {
        state
            .settings
            .read()
            .as_ref()
            .map(|s| (s.pickup_lat, s.pickup_lng))
            .unwrap_or((
                suki_shared::delivery::BANKEROHAN_LAT,
                suki_shared::delivery::BANKEROHAN_LNG,
            ))
    };
    let current = location();
    let destination = (order.lat, order.lng);
    use_effect(move || {
        let position = location();
        let map_data = serde_json::json!({"pickup":[pickup.0,pickup.1],"destination":[destination.0,destination.1],"rider":position.as_ref().map(|p| [p.lat,p.lng])});
        // Values are serialized numbers only, never executable source or user text.
        let js = format!(
            r#"
            (() => {{
                const data = {map_data};
                const initialize = (attempt) => {{
                    const node = document.getElementById('suki-live-map');
                    if (!node) return;
                    if (!window.L) {{ if (attempt < 50) setTimeout(() => initialize(attempt + 1), 100); return; }}
                    if (!node._sukiMap) {{
                        const map = L.map(node, {{scrollWheelZoom: false}});
                        L.tileLayer('https://tile.openstreetmap.org/{{z}}/{{x}}/{{y}}.png', {{maxZoom: 19, attribution: '&copy; OpenStreetMap contributors'}}).addTo(map);
                        node._sukiMap = map; node._sukiMarkers = {{}};
                    }}
                    const map = node._sukiMap;
                    for (const [key, label, color] of [['pickup','Pickup','#c78028'],['destination','Delivery','#d44750'],['rider','Rider','#18785d']]) {{
                        const point = data[key], old = node._sukiMarkers[key];
                        if (!point) {{ if (old) {{ map.removeLayer(old); delete node._sukiMarkers[key]; }} continue; }}
                        if (old) old.setLatLng(point);
                        else node._sukiMarkers[key] = L.circleMarker(point, {{radius: 9, color: '#fff', weight: 3, fillColor: color, fillOpacity: 1}}).addTo(map).bindPopup(label);
                    }}
                    if (!node._sukiFitted) {{ map.fitBounds([data.pickup, data.destination, ...(data.rider ? [data.rider] : [])], {{padding:[28,28], maxZoom:15}}); node._sukiFitted = true; }}
                }};
                initialize(0);
            }})();
        "#
        );
        document::eval(&js);
    });
    rsx! {
        section { class: "panel", h2 { "Follow your delivery" }
            p { class: "muted", "{status}" }
            div { id: "suki-live-map", class: "live-map", role: "application", aria_label: "Interactive delivery map" }
            p { class: "map-legend", "🟠 Pickup   🔴 Delivery   🟢 Rider" }
            if let Some(position) = current {
                p { "Rider: {position.lat:.5}, {position.lng:.5}" if now() - position.reported_at > 15 { " · Last report is stale" } }
            } else { p { "Waiting for the rider to share their location." } }
            if let Some(estimate) = eta() {
                p { strong { "About {(estimate.duration_seconds + 59) / 60} min" } " · {estimate.distance_meters} m remaining" }
            } else { p { class: "muted", "ETA unavailable until a fresh location and route are available." } }
            small { "Map tiles by OpenStreetMap; your map view is sent to its tile service." }
        }
    }
}

#[component]
pub fn Reviews(order: Order) -> Element {
    let api = use_context::<Api>();
    let mut reviews = use_signal(|| None::<OrderReviews>);
    let mut error = use_signal(String::new);
    let path = format!("/orders/{}/reviews", order.id);
    let fetch_path = path.clone();
    use_future(move || {
        let api = api.clone();
        let path = fetch_path.clone();
        async move {
            match api.get(&path).await {
                Ok(data) => reviews.set(Some(data)),
                Err(e) => error.set(e.to_string()),
            }
        }
    });
    rsx! {
        section { class: "panel", h2 { "How was your order?" }
            p { "Your feedback helps local stalls and riders grow." }
            if !error().is_empty() { p { role: "alert", "{error}" } }
            if let Some(data) = reviews() {
                if let Some(id) = order.vendor_id.clone() {
                    ReputationBadge { kind: "vendor".to_owned(), id, reviews }
                    ReviewForm { path: path.clone(), kind: "vendor".to_owned(), submitted: data.vendor.is_some(), reviews }
                }
                if let Some(id) = order.courier_id.clone() {
                    ReputationBadge { kind: "rider".to_owned(), id, reviews }
                    ReviewForm { path: path.clone(), kind: "rider".to_owned(), submitted: data.rider.is_some(), reviews }
                }
            } else if error().is_empty() { p { "Loading reviews…" } }
        }
    }
}

#[component]
fn ReviewForm(
    path: String,
    kind: String,
    submitted: bool,
    mut reviews: Signal<Option<OrderReviews>>,
) -> Element {
    let api = use_context::<Api>();
    let mut rating = use_signal(|| 0u8);
    let mut comment = use_signal(String::new);
    let mut tags = use_signal(Vec::<String>::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    let title = if kind == "vendor" { "Stall" } else { "Rider" };
    let choices = if kind == "vendor" {
        ["quality", "packaging", "accuracy"]
    } else {
        ["speed", "handling", "courtesy"]
    };
    rsx! {
        div { class: "review-form", h3 { "{title}" }
            if submitted { p { "Thanks! Your {title.to_lowercase()} review was submitted." } }
            else {
                p { "Choose a rating" }
                div { class: "rating-stars", role: "group", aria_label: "Rating for {title}",
                    for n in 1..=5u8 {
                        button { r#type: "button", class: if n <= rating() { "star selected" } else { "star" }, aria_label: "{n} stars", aria_pressed: rating() == n, onclick: move |_| rating.set(n), "★" }
                    }
                }
                div { class: "review-tags", aria_label: "Choose tags",
                    for tag in choices {
                        button { r#type: "button", aria_pressed: tags.read().iter().any(|value| value == tag), onclick: move |_| {
                            let mut selected = tags.write();
                            if let Some(i) = selected.iter().position(|value| value == tag) { selected.remove(i); } else { selected.push(tag.to_owned()); }
                        }, "{tag}" }
                    }
                }
                label { "Comment (optional)" textarea { maxlength: "1000", value: "{comment}", oninput: move |e| comment.set(e.value()) } }
                button { class: "primary", disabled: busy() || rating() == 0, onclick: move |_| {
                    if busy() || rating() == 0 { return; }
                    busy.set(true); error.set(String::new());
                    let body = ReviewInput { rating: rating(), comment: Some(comment()), tags: tags() };
                    let api = api.clone(); let path = path.clone(); let kind = kind.clone();
                    spawn(async move {
                        match api.send::<Acknowledged, _>(Method::POST, &format!("{path}/{kind}"), &body).await {
                            Ok(_) => match api.get(&path).await { Ok(data) => reviews.set(Some(data)), Err(e) => error.set(e.to_string()) },
                            Err(e) => error.set(e.to_string()),
                        }
                        busy.set(false);
                    });
                }, "Submit {title.to_lowercase()} review" }
                if !error().is_empty() { p { role: "alert", "{error}" } }
            }
        }
    }
}

#[component]
fn ReputationBadge(kind: String, id: String, reviews: Signal<Option<OrderReviews>>) -> Element {
    let api = use_context::<Api>();
    let mut score = use_signal(|| None::<Reputation>);
    use_resource(move || {
        let _ = reviews();
        let api = api.clone();
        let endpoint = format!("/reputation/{kind}/{id}");
        async move {
            score.set(api.get(&endpoint).await.ok());
        }
    });
    rsx! {
        if let Some(value) = score() {
            p { class: "muted",
                if value.total_reviews == 0 { "New · no ratings yet" }
                else { "★ {value.score} · {value.total_reviews} " if value.total_reviews == 1 { "review" } else { "reviews" } }
                " · {value.reputation_points} points"
            }
        }
    }
}
