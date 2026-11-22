//! Real MariaDB integration test. Run only against a dedicated test database.
use axum::{
    Json,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use suki_api::{App, AppState, id, now};
use tower::ServiceExt;

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    cookie: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("x-suki-request", "1")
        .header("origin", "http://localhost:3000")
        .header("cookie", cookie)
        .body(if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .to_string();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"raw":String::from_utf8_lossy(&bytes)}));
    (status, value, cookie)
}
async fn ok(app: &axum::Router, method: &str, path: &str, cookie: &str, body: Value) -> Value {
    let (status, value, _) = call(app, method, path, cookie, body).await;
    assert!(status.is_success(), "{method} {path}: {status}: {value}");
    value
}
async fn account(app: &axum::Router, role: &str) -> (String, String) {
    let email = format!("{}@test.invalid", id());
    let v=ok(app,"POST","/api/auth/register","",json!({"name":"Synthetic Test User","email":email,"phone":"09000000000","password":"Synthetic-test-password-42","role":role,"shop_name":"Test Fresh Picks"})).await;
    (v["id"].as_str().unwrap().into(), email)
}
async fn login(app: &axum::Router, email: &str) -> String {
    let (status, v, c) = call(
        app,
        "POST",
        "/api/auth/login",
        "",
        json!({"email":email,"password":"Synthetic-test-password-42"}),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert!(!c.is_empty());
    c
}
async fn activate(app: &axum::Router, admin: &str, uid: &str, role: &str) {
    ok(
        app,
        "PATCH",
        &format!("/api/admin/users/{uid}"),
        admin,
        json!({"role":role,"status":"active"}),
    )
    .await;
}
fn product(price: i64) -> Value {
    json!({"name":"Test Mangoes","local_name":"Mangga","description":"Synthetic fixture","category":"Fruits","unit":"kg","price_centavos":price,"image":"/images/mangoes.jpg","available":true})
}
async fn quote(app: &axum::Router, cookie: &str) -> Value {
    ok(
        app,
        "POST",
        "/api/delivery/quote",
        cookie,
        json!({"address":"123 Synthetic Street, Matina, Davao City","lat":7.05,"lng":125.59}),
    )
    .await
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing at a dedicated MariaDB database"]
async fn full_order_and_authorization() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL required");
    let db = sqlx::MySqlPool::connect(&url).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT DATABASE()")
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(name.ends_with("_test"), "Test database must end in _test");
    sqlx::migrate!("./migrations").run(&db).await.unwrap();
    let fixture = axum::Router::new().route(
        "/route",
        axum::routing::post(|Json(v): Json<Value>| async move {
            assert_eq!(v["travelMode"], "DRIVE");
            assert!(v.pointer("/destination/location/latLng/latitude").is_some());
            Json(json!({"routes":[{"distanceMeters":1234}]}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async { axum::serve(listener, fixture).await.unwrap() });
    let admin_email = format!("{}@test.invalid", id());
    let s: App = Arc::new(AppState {
        db: db.clone(),
        admin_emails: suki_api::auth::AdminEmails::parse(&format!(
            "  {}  , second@test.invalid",
            admin_email.to_uppercase()
        ))
        .unwrap(),
        integrity_key: suki_shared::crypto::IntegrityKey::from_hex(&"12".repeat(32)).unwrap(),
        client: reqwest::Client::new(),
        secure_cookies: false,
        origin: "http://localhost:3000".into(),
        maps_key: "synthetic-test-key".into(),
        maps_url: format!("http://{address}/route"),
        legacy_quotes: true,
        media_dir: std::env::temp_dir().join(format!("suki-media-{}", id())),
        events: tokio::sync::broadcast::channel(256).0,
    });
    let app = suki_api::router(s.clone());
    let uid = id();
    let phc = suki_api::auth::password_hash("Synthetic-test-password-42".into())
        .await
        .unwrap();
    sqlx::query("INSERT INTO users VALUES (?,?,?,'',?,'admin','active',?)")
        .bind(&uid)
        .bind("Test Admin")
        .bind(&admin_email)
        .bind(phc)
        .bind(now())
        .execute(&db)
        .await
        .unwrap();
    let admin = login(&app, &admin_email).await;
    let unknown_email = format!("{}@test.invalid", id());
    for email in [&admin_email, &unknown_email, &unknown_email] {
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/auth/login",
                "",
                json!({"email":email,"password":"Invalid-test-password-42"}),
            )
            .await
            .0,
            401
        );
    }

    let (customer_id, customer_email) = account(&app, "customer").await;
    let customer = login(&app, &customer_email).await;
    let (_, other_email) = account(&app, "customer").await;
    let other = login(&app, &other_email).await;
    let saved_address=ok(&app,"POST","/api/addresses",&customer,json!({"label":"Home","address":"123 Synthetic Address, Davao City","lat":7.05,"lng":125.59})).await;
    assert!(saved_address["id"].is_string());
    assert_eq!(
        ok(&app, "GET", "/api/addresses", &customer, Value::Null)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        ok(&app, "GET", "/api/addresses", &other, Value::Null)
            .await
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let unsafe_request = Request::builder()
        .method("POST")
        .uri("/api/auth/logout")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(unsafe_request).await.unwrap().status(),
        403
    );
    let foreign_origin = Request::builder()
        .method("POST")
        .uri("/api/auth/logout")
        .header("x-suki-request", "1")
        .header("origin", "https://untrusted.invalid")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(foreign_origin).await.unwrap().status(),
        403
    );
    let (vendor_id, vendor_email) = account(&app, "vendor").await;
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/auth/login",
            "",
            json!({"email":vendor_email,"password":"Synthetic-test-password-42"})
        )
        .await
        .0,
        403
    );
    activate(&app, &admin, &vendor_id, "vendor").await;
    let vendor = login(&app, &vendor_email).await;
    assert_eq!(
        call(&app, "POST", "/api/products", &customer, product(16000))
            .await
            .0,
        403
    );
    let pid = ok(&app, "POST", "/api/products", &vendor, product(16000)).await["id"]
        .as_u64()
        .unwrap();
    let (vendor2_id, vendor2_email) = account(&app, "vendor").await;
    activate(&app, &admin, &vendor2_id, "vendor").await;
    let vendor2 = login(&app, &vendor2_email).await;
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/api/products/{pid}"),
            &vendor2,
            product(1)
        )
        .await
        .0,
        403
    );
    // A stale availability action must never restore an older product price.
    ok(
        &app,
        "PUT",
        &format!("/api/products/{pid}"),
        &vendor,
        product(17000),
    )
    .await;
    for denied in [&vendor2, &customer] {
        assert_eq!(
            call(
                &app,
                "PATCH",
                &format!("/api/products/{pid}/availability"),
                denied,
                json!({"available":false})
            )
            .await
            .0,
            403
        );
    }
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/api/products/{pid}/availability"),
            &vendor,
            json!({"available":false,"price_centavos":16000})
        )
        .await
        .0,
        422
    );
    ok(
        &app,
        "PATCH",
        &format!("/api/products/{pid}/availability"),
        &vendor,
        json!({"available":false}),
    )
    .await;
    let (price, available): (i64, bool) =
        sqlx::query_as("SELECT price_centavos,available FROM products WHERE id=?")
            .bind(pid)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!((price, available), (17000, false));
    ok(
        &app,
        "PUT",
        &format!("/api/products/{pid}"),
        &vendor,
        product(16000),
    )
    .await;

    let (courier_id, courier_email) = account(&app, "customer").await;
    activate(&app, &admin, &courier_id, "courier").await;
    let courier = login(&app, &courier_email).await;
    let blocked_admin = suki_api::router(Arc::new(AppState {
        admin_emails: suki_api::auth::AdminEmails::parse(&format!(
            "{customer_email},{vendor_email},{courier_email}"
        ))
        .unwrap(),
        ..(*s).clone()
    }));
    for path in [
        "/api/admin/users",
        "/api/admin/reports",
        "/api/admin/audit",
        "/api/admin/orders/not-an-order/integrity",
        "/api/admin/payments/pending",
    ] {
        assert_eq!(call(&app, "GET", path, "", Value::Null).await.0, 401);
        for cookie in [&customer, &vendor, &courier, &admin] {
            assert_eq!(
                call(&blocked_admin, "GET", path, cookie, Value::Null)
                    .await
                    .0,
                403
            );
        }
    }
    assert_eq!(
        call(&blocked_admin, "PUT", "/api/settings", &admin, Value::Null)
            .await
            .0,
        403
    );
    assert_eq!(
        call(&blocked_admin, "GET", "/api/auth/me", &admin, Value::Null)
            .await
            .0,
        403
    );
    let (status, _, cookie) = call(
        &blocked_admin,
        "POST",
        "/api/auth/login",
        "",
        json!({"email":admin_email,"password":"Synthetic-test-password-42"}),
    )
    .await;
    assert_eq!(status, 403);
    assert!(cookie.is_empty());
    assert_eq!(
        ok(&app, "GET", "/api/auth/me", &admin, Value::Null).await["admin_access"],
        true
    );
    assert_eq!(
        ok(&app, "GET", "/api/auth/me", &customer, Value::Null).await["admin_access"],
        false
    );

    let mut settings = ok(&app, "GET", "/api/settings", "", Value::Null).await;
    settings["gcash_qr_url"] = json!("/images/synthetic-test-qr.png");
    ok(&app, "PUT", "/api/settings", &admin, settings).await;
    let unavailable: App = Arc::new(AppState {
        admin_emails: s.admin_emails.clone(),
        db: db.clone(),
        integrity_key: s.integrity_key.clone(),
        client: reqwest::Client::new(),
        secure_cookies: false,
        origin: "http://localhost:3000".into(),
        maps_key: String::new(),
        maps_url: "http://127.0.0.1:1".into(),
        legacy_quotes: true,
        media_dir: std::env::temp_dir().join(format!("suki-media-{}", id())),
        events: tokio::sync::broadcast::channel(4).0,
    });
    assert_eq!(
        call(
            &suki_api::router(unavailable),
            "POST",
            "/api/delivery/quote",
            &customer,
            json!({"address":"123 Synthetic Address, Davao City","lat":7.05,"lng":125.59})
        )
        .await
        .0,
        503
    );
    assert_eq!(
        ok(&app, "GET", "/api/health", "", Value::Null).await["status"],
        "ok"
    );
    let q = quote(&app, &customer).await;
    assert_eq!(q["distance_meters"], 1234);
    assert_eq!(q["delivery_fee_centavos"], 6432);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/delivery/quote",
            &customer,
            json!({"address":"Outside coverage test address","lat":10.0,"lng":125.0})
        )
        .await
        .0,
        400
    );
    let mut body = json!({"quote_id":q["id"],"idempotency_key":id(),"items":[{"product_id":pid,"quantity_milli":2000,"expected_price_centavos":16000}]});
    assert_eq!(
        call(&app, "POST", "/api/orders", &customer, body.clone())
            .await
            .0,
        400
    );
    body["items"][0]["quantity_milli"] = json!(3000);
    body["items"][0]["expected_price_centavos"] = json!(15999);
    assert_eq!(
        call(&app, "POST", "/api/orders", &customer, body.clone())
            .await
            .0,
        409
    );
    body["items"][0]["expected_price_centavos"] = json!(16000);
    let created = ok(&app, "POST", "/api/orders", &customer, body.clone()).await;
    let oid = created["id"].as_str().unwrap();
    let path = format!("/api/orders/{oid}");
    let replay = ok(&app, "POST", "/api/orders", &customer, body.clone()).await;
    assert_eq!(created["id"], replay["id"]);
    assert_eq!(replay["replayed"], true);
    body["items"][0]["quantity_milli"] = json!(4000);
    assert_eq!(
        call(&app, "POST", "/api/orders", &customer, body).await.0,
        409
    );
    assert_eq!(call(&app, "GET", &path, &other, Value::Null).await.0, 403);
    assert_eq!(call(&app, "GET", &path, &courier, Value::Null).await.0, 403);
    assert_eq!(call(&app, "GET", &path, "", Value::Null).await.0, 401);
    ok(
        &app,
        "PUT",
        &format!("/api/products/{pid}"),
        &vendor,
        product(20000),
    )
    .await;
    let detail = ok(&app, "GET", &path, &customer, Value::Null).await;
    assert_eq!(detail["items"][0]["unit_price_centavos"], 16000);
    assert_eq!(detail["order"]["estimated_total_centavos"], 54432);
    let item = detail["items"][0]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("{path}/status"),
            &admin,
            json!({"status":"shopping"})
        )
        .await
        .0,
        409
    );
    let reference = format!("TX{}", id().replace('-', ""));
    let payment = ok(
        &app,
        "POST",
        &format!("{path}/payments"),
        &customer,
        json!({"reference":reference,"amount_centavos":54432}),
    )
    .await;
    let payid = payment["id"].as_str().unwrap();
    let verify = format!("/api/payments/{payid}/verify");
    assert_eq!(payment["status"], "under_verification");
    // Pending evidence must remain verifiable, even if either role tries to cancel.
    for actor in [&customer, &admin] {
        assert_eq!(
            call(
                &app,
                "PATCH",
                &format!("{path}/status"),
                actor,
                json!({"status":"canceled"}),
            )
            .await
            .0,
            409
        );
    }
    let pending = ok(&app, "GET", &path, &customer, Value::Null).await;
    assert_eq!(pending["order"]["status"], "payment_verification");
    assert_eq!(pending["order"]["payment_status"], "under_verification");
    assert!(pending["refunds"].as_array().unwrap().is_empty());
    assert_eq!(
        call(
            &app,
            "POST",
            &verify,
            &customer,
            json!({"approved":true,"received_amount_centavos":54432})
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &verify,
            &admin,
            json!({"approved":true,"received_amount_centavos":54431})
        )
        .await
        .0,
        400
    );
    ok(
        &app,
        "POST",
        &verify,
        &admin,
        json!({"approved":true,"received_amount_centavos":54432}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "POST",
            &verify,
            &admin,
            json!({"approved":true,"received_amount_centavos":54432})
        )
        .await
        .0,
        409
    );
    ok(
        &app,
        "PUT",
        &format!("{path}/assign"),
        &admin,
        json!({"courier_id":courier_id}),
    )
    .await;
    ok(
        &app,
        "PATCH",
        &format!("{path}/status"),
        &courier,
        json!({"status":"shopping"}),
    )
    .await;
    assert_eq!(call(&app,"PUT",&format!("{path}/items/{item}/purchase"),&courier,json!({"actual_quantity_milli":3000,"actual_unit_price_centavos":20000,"unavailable":false})).await.0,409);
    let requested = ok(&app, "GET", "/api/vendor/items", &vendor, Value::Null).await;
    assert!(
        requested
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["item_id"] == item)
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/api/vendor/items/{item}"),
            &vendor2,
            json!({"available_quantity_milli":3000})
        )
        .await
        .0,
        403
    );
    ok(
        &app,
        "PUT",
        &format!("/api/vendor/items/{item}"),
        &vendor,
        json!({"available_quantity_milli":3000}),
    )
    .await;
    // The courier proposes a substitution. No purchase can be recorded before approval.
    let sub=ok(&app,"POST",&format!("{path}/substitutions"),&courier,json!({"item_id":item,"name":"Alternative mangoes","unit":"kg","quantity_milli":3000,"unit_price_centavos":15000})).await;
    assert_eq!(call(&app,"PUT",&format!("{path}/items/{item}/purchase"),&courier,json!({"actual_quantity_milli":3000,"actual_unit_price_centavos":15000,"unavailable":false})).await.0,409);
    ok(
        &app,
        "POST",
        &format!("{path}/substitutions/{}", sub["id"].as_str().unwrap()),
        &customer,
        json!({"approved":true}),
    )
    .await;
    ok(&app,"PUT",&format!("{path}/items/{item}/purchase"),&courier,json!({"actual_quantity_milli":3000,"actual_unit_price_centavos":15000,"unavailable":false})).await;
    let rec = ok(
        &app,
        "POST",
        &format!("{path}/reconcile"),
        &courier,
        Value::Null,
    )
    .await;
    assert_eq!(rec["status"], "ready_for_delivery");
    assert_eq!(rec["difference_centavos"], -3000);
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}/messages"),
            &other,
            Value::Null
        )
        .await
        .0,
        403
    );
    ok(
        &app,
        "POST",
        &format!("{path}/messages"),
        &customer,
        json!({"content":"Please ring the bell."}),
    )
    .await;
    let messages = ok(
        &app,
        "GET",
        &format!("{path}/messages?after=0"),
        &courier,
        Value::Null,
    )
    .await;
    assert_eq!(messages[0]["content"], "Please ring the bell.");
    assert_eq!(messages[0]["sender_id"], customer_id);
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}/location"),
            &customer,
            Value::Null
        )
        .await
        .0,
        409
    );
    ok(
        &app,
        "PATCH",
        &format!("{path}/status"),
        &courier,
        json!({"status":"out_for_delivery"}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("{path}/location"),
            &other,
            json!({"lat":7.05,"lng":125.59})
        )
        .await
        .0,
        403
    );
    ok(
        &app,
        "PUT",
        &format!("{path}/location"),
        &courier,
        json!({"lat":7.05,"lng":125.59}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("{path}/location"),
            &courier,
            json!({"lat":7.05,"lng":125.59})
        )
        .await
        .0,
        429
    );
    let location = ok(
        &app,
        "GET",
        &format!("{path}/location"),
        &customer,
        Value::Null,
    )
    .await;
    assert_eq!(location["lat"], 7.05);
    ok(
        &app,
        "PATCH",
        &format!("{path}/status"),
        &courier,
        json!({"status":"delivered"}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}/location"),
            &customer,
            Value::Null
        )
        .await
        .0,
        409
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delivery_locations WHERE order_id=?")
        .bind(oid)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let detail = ok(&app, "GET", &path, &admin, Value::Null).await;
    assert_eq!(detail["refunds"][0][1], 3000);
    let rid = detail["refunds"][0][0].as_str().unwrap();
    ok(
        &app,
        "POST",
        &format!("/api/refunds/{rid}/process"),
        &admin,
        json!({"reference":"SYNTHETIC-REFUND-42"}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/refunds/{rid}/process"),
            &admin,
            json!({"reference":"SYNTHETIC-REFUND-42"})
        )
        .await
        .0,
        409
    );
    ok(
        &app,
        "PATCH",
        &format!("{path}/status"),
        &courier,
        json!({"status":"completed"}),
    )
    .await;
    let reports = ok(&app, "GET", "/api/admin/reports", &admin, Value::Null).await;
    assert!(
        reports["orders"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["order_id"] == oid && r["margin_centavos"] == 6432)
    );
    assert_eq!(
        call(&app, "GET", "/api/admin/reports", &customer, Value::Null)
            .await
            .0,
        403
    );
    // Exercise a real authenticated WebSocket handshake and event delivery.
    use futures_util::StreamExt;
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{Message as WsMessage, client::IntoClientRequest},
    };
    let live_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let live_address = live_listener.local_addr().unwrap();
    let live_app = app.clone();
    let live_task =
        tokio::spawn(async move { axum::serve(live_listener, live_app).await.unwrap() });
    let ws_url = format!("ws://{live_address}{path}/events");
    let mut forbidden = ws_url.clone().into_client_request().unwrap();
    forbidden
        .headers_mut()
        .insert("origin", "http://localhost:3000".parse().unwrap());
    forbidden
        .headers_mut()
        .insert("cookie", other.parse().unwrap());
    assert!(connect_async(forbidden).await.is_err());
    let mut wrong_origin = ws_url.clone().into_client_request().unwrap();
    wrong_origin
        .headers_mut()
        .insert("origin", "https://untrusted.invalid".parse().unwrap());
    wrong_origin
        .headers_mut()
        .insert("cookie", customer.parse().unwrap());
    assert!(connect_async(wrong_origin).await.is_err());
    let mut request = ws_url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", "http://localhost:3000".parse().unwrap());
    request
        .headers_mut()
        .insert("cookie", customer.parse().unwrap());
    let (mut ws, _) = connect_async(request).await.unwrap();
    ok(
        &app,
        "POST",
        &format!("{path}/messages"),
        &courier,
        json!({"content":"Your delivery is complete."}),
    )
    .await;
    let event = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Some(Ok(WsMessage::Text(text))) = ws.next().await {
                break serde_json::from_str::<Value>(&text).unwrap();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(event["type"], "message");
    assert_eq!(event["message"]["content"], "Your delivery is complete.");
    let resumed = ok(
        &app,
        "GET",
        &format!("{path}/messages?after={}", messages[0]["id"]),
        &customer,
        Value::Null,
    )
    .await;
    assert_eq!(resumed[0]["content"], "Your delivery is complete.");

    // Reusing a verified payment reference on another order must fail.
    let q2 = quote(&app, &customer).await;
    let second_body = json!({"quote_id":q2["id"],"idempotency_key":id(),"items":[{"product_id":pid,"quantity_milli":3000,"expected_price_centavos":20000}]});
    let (one, two) = tokio::join!(
        call(&app, "POST", "/api/orders", &customer, second_body.clone()),
        call(&app, "POST", "/api/orders", &customer, second_body)
    );
    assert_eq!(one.0, 200);
    assert_eq!(two.0, 200);
    assert_eq!(one.1["id"], two.1["id"]);
    let oid2 = one.1["id"].as_str().unwrap();
    let path2 = format!("/api/orders/{oid2}");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path2}/payments"),
            &customer,
            json!({"reference":format!("  {}  ",reference.to_lowercase()),"amount_centavos":66432})
        )
        .await
        .0,
        409
    );
    let p2 = ok(
        &app,
        "POST",
        &format!("{path2}/payments"),
        &customer,
        json!({"reference":format!("TX{}",id().replace('-',"")),"amount_centavos":66432}),
    )
    .await;
    ok(
        &app,
        "POST",
        &format!("/api/payments/{}/verify", p2["id"].as_str().unwrap()),
        &admin,
        json!({"approved":true,"received_amount_centavos":66432}),
    )
    .await;
    ok(
        &app,
        "PUT",
        &format!("{path2}/assign"),
        &admin,
        json!({"courier_id":courier_id}),
    )
    .await;
    ok(
        &app,
        "PATCH",
        &format!("{path2}/status"),
        &courier,
        json!({"status":"shopping"}),
    )
    .await;
    let adjustment=ok(&app,"POST",&format!("{path2}/adjustments"),&courier,json!({"proposed_goods_centavos":63000,"reason":"Vendor reports higher actual weight; ask before buying."})).await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path2}/adjustments/{}", adjustment["id"].as_str().unwrap()),
            &other,
            json!({"approved":true})
        )
        .await
        .0,
        403
    );
    ok(
        &app,
        "POST",
        &format!("{path2}/adjustments/{}", adjustment["id"].as_str().unwrap()),
        &customer,
        json!({"approved":true}),
    )
    .await;
    let p3 = ok(
        &app,
        "POST",
        &format!("{path2}/payments"),
        &customer,
        json!({"reference":format!("TX{}",id().replace('-',"")),"amount_centavos":3000}),
    )
    .await;
    ok(
        &app,
        "POST",
        &format!("/api/payments/{}/verify", p3["id"].as_str().unwrap()),
        &admin,
        json!({"approved":true,"received_amount_centavos":3000}),
    )
    .await;
    let second_detail = ok(&app, "GET", &path2, &customer, Value::Null).await;
    let second_item = second_detail["items"][0]["id"].as_str().unwrap();
    ok(&app,"PUT",&format!("{path2}/items/{second_item}/purchase"),&courier,json!({"actual_quantity_milli":3000,"actual_unit_price_centavos":21000,"unavailable":false})).await;
    assert_eq!(
        ok(
            &app,
            "POST",
            &format!("{path2}/reconcile"),
            &courier,
            Value::Null
        )
        .await["status"],
        "ready_for_delivery"
    );
    ok(
        &app,
        "PATCH",
        &format!("{path2}/status"),
        &admin,
        json!({"status":"canceled"}),
    )
    .await;
    let canceled = ok(&app, "GET", &path2, &customer, Value::Null).await;
    assert_eq!(canceled["refunds"][0][1], 69432);
    marketplace_workflow(
        &app,
        s.as_ref(),
        live_address,
        [&customer, &vendor, &courier, &admin, &vendor2],
        &courier_id,
    )
    .await;

    activate(&app, &admin, &vendor2_id, "vendor").await;
    assert_eq!(
        call(&app, "GET", "/api/auth/me", &vendor2, Value::Null)
            .await
            .0,
        401
    );

    ok(&app, "POST", "/api/auth/logout", &customer, Value::Null).await;
    assert_eq!(
        call(&app, "GET", &path, &customer, Value::Null).await.0,
        401
    );
    // Every accepted critical action is audited; passwords and session tokens are not returned.
    let logs = ok(&app, "GET", "/api/admin/audit", &admin, Value::Null).await;
    assert!(
        logs.as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"] == "payment.verify")
    );
    live_task.abort();
    task.abort();
    db.close().await;
}

async fn marketplace_workflow(
    app: &axum::Router,
    state: &AppState,
    address: std::net::SocketAddr,
    [customer, vendor, rider, admin, stranger]: [&str; 5],
    rider_id: &str,
) {
    use futures_util::StreamExt;
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{Message, client::IntoClientRequest},
    };
    let mut buffer = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(3, 2, image::Rgb([160, 20, 60]))
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    let fields = json!({
        "name": "Ripe Mangoes", "description":"A vendor listing", "category":"Fruits",
        "unit":"bag", "price_centavos":24000, "weight_grams":2000
    });
    let boundary = "suki-integration-upload";
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"product\"\r\n\r\n{fields}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"../../sample.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
    body.extend(buffer.into_inner());
    body.extend(format!("\r\n--{boundary}--\r\n").bytes());
    let upload = Request::post("/api/vendor/goods")
        .header("cookie", vendor)
        .header("x-suki-request", "1")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(upload).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let uploaded: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let pid = uploaded["id"].as_i64().unwrap();
    let (path, grams): (String, i64) =
        sqlx::query_as("SELECT image,weight_grams FROM products WHERE id=?")
            .bind(pid)
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(grams, 2000);
    let photo = app
        .clone()
        .oneshot(Request::get(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(photo.headers()["content-type"], "image/jpeg");
    let bytes = photo.into_body().collect().await.unwrap().to_bytes();
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (3, 2));
    assert!(
        state
            .media_dir
            .join(path.strip_prefix("/images/vendor/").unwrap())
            .is_file()
    );

    let price_path = format!("/api/vendor/goods/{pid}/price");
    assert_eq!(
        call(
            app,
            "PATCH",
            &price_path,
            stranger,
            json!({"price_centavos":17000})
        )
        .await
        .0,
        403
    );
    ok(
        app,
        "PATCH",
        &price_path,
        vendor,
        json!({"price_centavos":25000}),
    )
    .await;
    ok(
        app,
        "PATCH",
        &format!("/api/vendor/goods/{pid}/availability"),
        vendor,
        json!({"available":true}),
    )
    .await;
    let make_quote = |grams| {
        json!({"address":"123 Synthetic Street, Matina, Davao City",
        "lat":7.05,"lng":125.59,"weight_grams":grams})
    };
    let make_checkout = |quote: Value, payment| {
        json!({"quote_id":quote["id"],"idempotency_key":id(),
        "payment_method":payment,"items":[{"product_id":pid,"quantity_milli":2000,"expected_price_centavos":25000}]})
    };
    let too_light = ok(
        app,
        "POST",
        "/api/quotes/delivery",
        customer,
        make_quote(1000),
    )
    .await;
    assert_eq!(
        call(
            app,
            "POST",
            "/api/orders",
            customer,
            make_checkout(too_light, "cash_on_delivery")
        )
        .await
        .0,
        409
    );
    let quote = ok(
        app,
        "POST",
        "/api/quotes/delivery",
        customer,
        make_quote(4000),
    )
    .await;
    assert_eq!(
        (
            quote["delivery_fee_centavos"].as_i64(),
            quote["weight_grams"].as_i64()
        ),
        (Some(8500), Some(4000))
    );
    let checkout_body = make_checkout(quote, "cash_on_delivery");
    let order = ok(
        app,
        "POST",
        "/api/orders/checkout",
        customer,
        checkout_body.clone(),
    )
    .await;
    let oid = order["id"].as_str().unwrap();
    let path = format!("/api/orders/{oid}");
    let receipt_path = format!("{path}/receipt");
    let receipt: suki_shared::snapshots::OrderSnapshot =
        serde_json::from_value(order["snapshot"].clone()).unwrap();
    assert!(receipt.valid_digest_for(oid));
    assert!(state.integrity_key.verify(
        suki_shared::crypto::SNAPSHOT_AUTH,
        &receipt.snapshot_hash,
        &receipt.signature
    ));
    assert_eq!(receipt.payload.total_centavos, 58500);
    assert_eq!(receipt.payload.delivery.total_centavos, 8500);
    assert_eq!(receipt.payload.delivery.distance_meters, 1234);
    assert_eq!(receipt.payload.delivery.weight_grams, Some(4000));
    assert_eq!(receipt.payload.items[0].unit_price_centavos, 25000);
    assert_eq!(receipt.payload.items[0].unit_weight_grams, 2000);

    // Changing a live listing cannot change the signed receipt or the vendor's locked price.
    ok(
        app,
        "PATCH",
        &price_path,
        vendor,
        json!({"price_centavos":26000}),
    )
    .await;
    ok(
        app,
        "PATCH",
        &format!("/api/vendor/goods/{pid}/availability"),
        vendor,
        json!({"available":false}),
    )
    .await;
    assert_eq!(
        ok(app, "GET", &receipt_path, customer, Value::Null).await,
        order["snapshot"]
    );
    for account in [vendor, rider, stranger] {
        assert_eq!(
            call(app, "GET", &receipt_path, account, Value::Null)
                .await
                .0,
            403
        );
    }
    let vendor_queue = ok(app, "GET", "/api/vendor/items", vendor, Value::Null).await;
    let line = vendor_queue
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["order_id"] == oid)
        .unwrap();
    assert_eq!(line["unit_price_centavos"], 25000);
    assert_eq!(line["line_total_centavos"], 50000);
    assert_eq!(line["snapshot_verified"], true);
    assert!(line.get("address").is_none());
    assert!(line.get("payment_method").is_none());

    let replay = ok(
        app,
        "POST",
        "/api/orders/checkout",
        customer,
        checkout_body.clone(),
    )
    .await;
    assert_eq!(replay["id"], oid);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["snapshot"], order["snapshot"]);

    // If receipt authentication fails, a retry must still recover the committed order.
    let wrong_key_app = suki_api::router(Arc::new(AppState {
        integrity_key: suki_shared::crypto::IntegrityKey::from_hex(&"ab".repeat(32)).unwrap(),
        ..state.clone()
    }));
    let recovered = ok(
        &wrong_key_app,
        "POST",
        "/api/orders/checkout",
        customer,
        checkout_body,
    )
    .await;
    assert_eq!(recovered["id"], oid);
    assert_eq!(recovered["replayed"], true);
    assert_eq!(recovered["snapshot_invalid"], true);
    assert!(recovered["snapshot"].is_null());
    assert_eq!(
        call(&wrong_key_app, "GET", &receipt_path, customer, Value::Null)
            .await
            .0,
        409
    );
    assert!(
        sqlx::query("UPDATE order_snapshots SET payload='{}' WHERE order_id=?")
            .bind(oid)
            .execute(&state.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM order_snapshots WHERE order_id=?")
            .bind(oid)
            .execute(&state.db)
            .await
            .is_err()
    );

    ok(
        app,
        "PATCH",
        &price_path,
        vendor,
        json!({"price_centavos":25000}),
    )
    .await;
    ok(
        app,
        "PATCH",
        &format!("/api/vendor/goods/{pid}/availability"),
        vendor,
        json!({"available":true}),
    )
    .await;
    let assign = format!("{path}/assign");
    let status = format!("{path}/status");
    ok(app, "PUT", &assign, admin, json!({"courier_id":rider_id})).await;
    assert_eq!(call(app, "GET", &path, vendor, Value::Null).await.0, 403);
    assert_eq!(
        call(
            app,
            "GET",
            &format!("{path}/messages"),
            stranger,
            Value::Null
        )
        .await
        .0,
        403
    );
    let ws_url = format!("ws://{address}{path}/chat/ws");
    let connect = |cookie: &str| {
        let mut request = ws_url.as_str().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("origin", "http://localhost:3000".parse().unwrap());
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
        connect_async(request)
    };
    assert!(connect(stranger).await.is_err());
    let mut sockets = Vec::new();
    for participant in [customer, vendor, rider] {
        let (mut socket, _) = connect(participant).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
                .await
                .unwrap(),
            Some(Ok(Message::Ping(_)))
        ));
        sockets.push(socket);
    }
    for (sender, role) in [
        (vendor, "vendor"),
        (customer, "customer"),
        (rider, "courier"),
    ] {
        let content = format!("Pickup update from {role}");
        ok(
            app,
            "POST",
            &format!("{path}/messages"),
            sender,
            json!({"content":content}),
        )
        .await;
        for socket in &mut sockets {
            let response = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while let Some(Ok(frame)) = socket.next().await {
                    if let Message::Text(text) = frame {
                        return serde_json::from_str::<Value>(&text).unwrap();
                    }
                }
                panic!("Socket closed without broadcasting the order message");
            })
            .await
            .unwrap();
            assert_eq!(response["message"]["content"], content);
            assert_eq!(response["message"]["sender_role"], role);
            assert_eq!(response["message"]["order_id"], oid);
            let mut message: suki_shared::realtime::ChatMessage =
                serde_json::from_value(response["message"].clone()).unwrap();
            assert!(suki_api::integrity::valid_message(
                &state.integrity_key,
                &message
            ));
            message.content.push('!');
            assert!(!suki_api::integrity::valid_message(
                &state.integrity_key,
                &message
            ));
        }
    }

    assert_eq!(
        call(
            app,
            "POST",
            &format!("{path}/payments"),
            customer,
            json!({"reference":"COD-CANNOT","amount_centavos":58500})
        )
        .await
        .0,
        409
    );
    ok(app, "PATCH", &status, rider, json!({"status":"shopping"})).await;
    let detail = ok(app, "GET", &path, rider, Value::Null).await;
    let line_id = detail["items"][0]["id"].as_str().unwrap();
    ok(app, "PUT", &format!("{path}/items/{line_id}/purchase"), rider,
        json!({"actual_quantity_milli":2000,"actual_unit_price_centavos":24500,"unavailable":false})).await;
    let reconciliation = ok(
        app,
        "POST",
        &format!("{path}/reconcile"),
        rider,
        Value::Null,
    )
    .await;
    assert_eq!(reconciliation["final_goods_centavos"], 49000);
    ok(
        app,
        "PATCH",
        &status,
        rider,
        json!({"status":"out_for_delivery"}),
    )
    .await;
    assert_eq!(
        call(app, "PATCH", &status, rider, json!({"status":"delivered"}))
            .await
            .0,
        409
    );
    let collection = format!("{path}/cash-collection");
    assert_eq!(
        call(
            app,
            "POST",
            &collection,
            rider,
            json!({"amount_centavos":57499})
        )
        .await
        .0,
        400
    );
    ok(
        app,
        "POST",
        &collection,
        rider,
        json!({"amount_centavos":57500}),
    )
    .await;
    assert_eq!(
        call(
            app,
            "POST",
            &collection,
            rider,
            json!({"amount_centavos":57500})
        )
        .await
        .0,
        409
    );
    ok(app, "PATCH", &status, rider, json!({"status":"delivered"})).await;
    assert_eq!(
        call(app, "PATCH", &status, rider, json!({"status":"completed"}))
            .await
            .0,
        409
    );
    ok(
        app,
        "POST",
        &format!("{path}/vendor-settlement"),
        rider,
        json!({"reference":"VENDOR-COD-001"}),
    )
    .await;
    ok(app, "PATCH", &status, rider, json!({"status":"completed"})).await;
    let delivered = ok(app, "GET", &path, customer, Value::Null).await;
    assert_eq!(delivered["snapshot"], order["snapshot"]);
    assert_eq!(delivered["settlement"]["amount_centavos"], 49000);
    assert_eq!(delivered["payments"][0]["received_amount_centavos"], 57500);

    ok(
        app,
        "PUT",
        "/api/vendor/payment-profile",
        vendor,
        json!({"gcash_number":"09123456789"}),
    )
    .await;
    let quote = ok(
        app,
        "POST",
        "/api/quotes/delivery",
        customer,
        make_quote(4000),
    )
    .await;
    let direct = ok(
        app,
        "POST",
        "/api/orders",
        customer,
        make_checkout(quote, "gcash"),
    )
    .await;
    let direct = format!("/api/orders/{}", direct["id"].as_str().unwrap());
    let detail = ok(app, "GET", &direct, customer, Value::Null).await;
    assert_eq!(detail["order"]["gcash_number"], "09123456789");
    ok(
        app,
        "PUT",
        &format!("{direct}/assign"),
        admin,
        json!({"courier_id":rider_id}),
    )
    .await;
    let reference = format!("GCASH-{}", id());
    let evidence = ok(
        app,
        "POST",
        &format!("{direct}/payments"),
        customer,
        json!({"reference":reference,"amount_centavos":58500}),
    )
    .await;
    let queue = ok(
        app,
        "GET",
        "/api/admin/payments/pending",
        admin,
        Value::Null,
    )
    .await;
    assert!(
        queue
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == evidence["id"]
                && p["reference_hash"] == suki_shared::crypto::gcash_reference_hash(&reference))
    );
    ok(
        app,
        "POST",
        &format!("/api/payments/{}/verify", evidence["id"].as_str().unwrap()),
        rider,
        json!({"approved":true,"received_amount_centavos":58500}),
    )
    .await;
    assert_eq!(
        ok(app, "GET", &direct, customer, Value::Null).await["order"]["status"],
        "paid"
    );
    let integrity_path = format!("/api/admin/orders/{oid}/integrity");
    assert_eq!(
        call(app, "GET", &integrity_path, customer, Value::Null)
            .await
            .0,
        403
    );
    let report = ok(app, "GET", &integrity_path, admin, Value::Null).await;
    assert_eq!(report["messages_verified"], 3);
    assert_eq!(report["snapshot"], "verified");
    assert_eq!(report["legacy_messages"], 0);
    assert!(report["transactions_verified"].as_u64().unwrap() >= 8);
    assert!(report["first_invalid_transaction"].is_null());
    assert!(report["first_invalid_message"].is_null());
    let entries = report["recent"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry["event"] == "payments.insert")
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["event"] == "vendor_settlements.insert")
    );
    assert!(
        sqlx::query("UPDATE order_integrity SET payload='{}' WHERE order_id=?")
            .bind(oid)
            .execute(&state.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM order_integrity WHERE order_id=?")
            .bind(oid)
            .execute(&state.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE chat_messages SET content='changed' WHERE order_id=?")
            .bind(oid)
            .execute(&state.db)
            .await
            .is_err()
    );

    // Confirm this paid order is assignable before testing the integrity guard.
    let assignment = format!("{direct}/assign");
    let rider_assignment = json!({"courier_id":rider_id});
    ok(app, "PUT", &assignment, admin, rider_assignment.clone()).await;
    let paid_order_id = detail["order"]["id"].as_str().unwrap();
    let integrity_path = format!("/api/admin/orders/{paid_order_id}/integrity");
    let before = ok(app, "GET", &integrity_path, admin, Value::Null).await;
    assert!(before["first_invalid_transaction"].is_null());

    // A database edit creates an unsigned entry. A later API action must not seal it.
    sqlx::query("UPDATE orders SET delivery_fee_centavos=delivery_fee_centavos+1 WHERE id=?")
        .bind(paid_order_id)
        .execute(&state.db)
        .await
        .unwrap();
    let report = ok(app, "GET", &integrity_path, admin, Value::Null).await;
    assert!(report["first_invalid_transaction"].is_number());
    let (status, error, _) = call(app, "PUT", &assignment, admin, rider_assignment).await;
    assert_eq!(status, 409);
    assert_eq!(error["error"], "Order integrity needs administrator review");
    let after = ok(app, "GET", &integrity_path, admin, Value::Null).await;
    assert_eq!(
        report["first_invalid_transaction"],
        after["first_invalid_transaction"]
    );

    tokio::fs::remove_dir_all(&state.media_dir).await.unwrap();
}
