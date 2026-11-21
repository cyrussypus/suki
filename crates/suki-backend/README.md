# Suki API

See the [setup guide](../../docs/SETUP.md) for database creation and native app builds. To start the API, copy `.env.example` to `.env` in this directory, set private environment values, and run `cargo run --locked`.

Startup applies the versioned migrations before listening. The bootstrap administrator is created only when no administrator exists. New vendors remain pending until activated. Public registration cannot create administrators or couriers. An administrator may promote an existing account to courier or be assigned to an order.

The API binary is `suki-api`. Its image directory can be configured with `SUKI_IMAGE_DIR`. The optional development preview and the native client use the same API endpoints.

## Security and storage

- Argon2id password hashes; bounded password workers; per-identity authentication attempt limits.
- Random opaque session cookies, HttpOnly, SameSite=Lax, optional development-only insecure mode. Only SHA-256 token hashes are stored. Sessions expire after seven days, logout removes them, and administrative account changes revoke them.
- Mutating requests require `X-Suki-Request: 1`. Browser Origin must match the configured origin; no cross-origin CORS access is granted. WebSockets require an exact Origin and a session cookie.
- SQL values are bound parameters. SQLx 0.9 additionally rejects dynamically composed unaudited query strings. HTTP bodies are limited to 32 KiB; messages and domain input have narrower limits.
- Only customers can create their own orders, only vendors can modify their own products, and only order participants or administrators can read private order data. Vendors see only their own requested item lines, not customer addresses or payments.
- All money is integer centavos. Quantities use thousandths of the listed unit (`quantity_milli=1500` means 1.5 kg when unit is kg). Price multiplication and final fees round half up. Original product names, units, prices and quantities remain immutable snapshots.
- Checkout uses an expiring customer-owned server quote, locks relevant records, revalidates product prices and availability, and enforces the configured minimum excluding delivery. The same idempotency key and payload returns the same order; a changed payload with that key conflicts.
- Payment evidence never marks an order paid. Administrators must confirm the exact received outstanding amount. Transaction references are globally unique. Shopping requires paid status. Spending increases and substitutions require explicit customer approval; additional funds require a second manual verification before shopping resumes.
- Reconciliation records actual costs separately from estimates, creates refund obligations, and unlocks delivery only after all items are recorded. Refund processing is a manually recorded external operation, with duplicate processing blocked.
- Chat messages persist before broadcasts. REST cursors retrieve missed messages. WebSocket subscriptions recheck sessions and order access on each event and heartbeat. Client reconnects with backoff and also polls as fallback. GPS updates require the assigned courier, active delivery, valid coordinates and at least three seconds between updates. Only the latest location is retained; it is deleted on delivery/cancellation. Location reads are denied afterward.
- Settings, account changes, payment verification, refunds, purchases and order transitions are audited. Logging excludes request bodies and credentials.

Deploy one backend instance for the initial dedicated-courier MVP: broadcasts are in-process. Multiple instances require a shared event broker; MariaDB remains the persistence source.

## API

All paths begin `/api`. Authenticated calls use `suki_session`; mutations require the custom request header. Monetary JSON fields end in `_centavos`; timestamps are UTC Unix seconds. Error responses contain `{ "error": "..." }`. IDs are UUID strings except product/message IDs. Lists are bounded; order/admin listings are currently capped at 100/500 entries.

| Method | Path | Access / body |
| --- | --- | --- |
| GET | `/health` | Database readiness |
| POST | `/auth/register` | `{name,email,phone,password,role,shop_name?}`; role customer/vendor, password 12–128 bytes |
| POST | `/auth/login` | `{email,password}`; sets session cookie |
| POST | `/auth/logout` | Deletes current session |
| GET | `/auth/me` | Current account |
| GET, POST | `/products` | Public read; vendor create `{name,local_name,description,category,unit,price_centavos,image,available}` |
| PUT | `/products/{id}` | Owning vendor; same product body |
| GET, POST | `/categories` | Public list; admin create `{name}` |
| GET, POST | `/addresses` | Customer list/create `{label,address,lat,lng}` |
| GET | `/vendor/items` | Vendor's paid active order item lines |
| PUT | `/vendor/items/{id}` | Owning vendor `{available_quantity_milli}` |
| GET, PUT | `/settings` | Public read; admin update complete settings object from GET |
| POST | `/delivery/quote` | Customer `{address,lat,lng}`; Google driving distance; ten-minute quote |
| GET, POST | `/orders` | Authorized list; customer checkout `{quote_id,idempotency_key,items:[{product_id,quantity_milli,expected_price_centavos}]}` |
| GET | `/orders/{id}` | Order, immutable items, payments, refunds, adjustments, substitutions and status history |
| PUT | `/orders/{id}/assign` | Admin `{courier_id}` for paid active orders |
| PATCH | `/orders/{id}/status` | `{status}`; valid transition and actor required |
| PUT | `/orders/{id}/items/{item}/purchase` | Assigned courier/admin `{actual_quantity_milli,actual_unit_price_centavos,unavailable}` |
| POST | `/orders/{id}/adjustments` | Courier/admin `{proposed_goods_centavos,reason}` before exceeding spending authorization |
| POST | `/orders/{id}/adjustments/{adjustment}` | Customer `{approved}`; reject then operator resolves/cancels |
| POST | `/orders/{id}/substitutions` | Courier/admin `{item_id,name,unit,quantity_milli,unit_price_centavos}`; one proposal per item |
| POST | `/orders/{id}/substitutions/{substitution}` | Customer `{approved}` |
| POST | `/orders/{id}/reconcile` | Courier/admin; all actual item records required |
| POST | `/orders/{id}/payments` | Customer `{reference,amount_centavos}` for outstanding authorized amount |
| POST | `/payments/{id}/verify` | Admin `{approved,received_amount_centavos}`; rejected evidence uses zero received |
| POST | `/refunds/{id}/process` | Admin `{reference}` after externally processing refund |
| GET, POST | `/orders/{id}/messages` | Participants; GET `?after=<message-id>` (200 per page), POST `{content}` |
| GET, PUT | `/orders/{id}/location` | Eligible participant read; assigned courier `{lat,lng}` write |
| GET | `/orders/{id}/events` | Authenticated WebSocket; server events `message`, `location`, `status`, `resync`; writes use REST |
| GET | `/admin/users` | Admin user management |
| PATCH | `/admin/users/{id}` | Admin `{role,status}`; cannot change own access |
| POST | `/orders/{id}/expenses` | Courier/admin `{amount_centavos,description}` |
| GET | `/admin/reports`, `/admin/audit` | Admin profitability, status counts and audit entries |

<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:e559a3fa94da3c884fc7a99fd53ee583dd2e8661915e5e281d749bd0d36f76f8 -->

<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:6c16b7e0644d3d9a8e176375209a8d6a4f7136f21bde0c9e2eebf287ca2a4c23 -->

The owning vendor can change stock availability with `PATCH /api/products/{id}/availability` and `{available}`. This leaves other product fields unchanged.

<!-- /coderabbit-ai-generated:v1 -->

<!-- /coderabbit-ai-generated:v1 -->

Principal flow: `awaiting_payment → payment_verification → paid → shopping → ready_for_delivery → out_for_delivery → delivered → completed`. A higher spending authorization pauses shopping in `price_adjustment`; customer approval and verified additional payment resume it. Reconciliation sets `ready_for_delivery`; clients cannot jump directly there. Customers may cancel before shopping; administrators may cancel before delivery completion, creating the remaining refund obligation. Both must wait until pending payment evidence is verified or rejected before canceling.

## Google Maps and GCash

Configure a restricted server `GOOGLE_MAPS_API_KEY` with Google Routes `directions/v2:computeRoutes`. The API sends driving origin/destination coordinates and requests `routes.distanceMeters`. It validates coordinate ranges, configured bounding-box coverage and maximum road distance. The default origin is Bankerohan. Administrators must review the origin and coverage against actual operations before launch.

Fee: `base_fee_centavos + round(per_km_centavos × distance_meters / 1000)`. Defaults are ₱60 + ₱3.50/km and ₱450 goods. The app uses saved or manually entered destination coordinates. A device GPS picker and interactive map are future work.

Upload the operator's actual GCash QR as a local `/images/...` asset and set `gcash_qr_url` through the admin settings API. Do not substitute a sample QR. Checkout fails closed if routing or the configured QR is missing. Evidence is a reference number; screenshot upload is not implemented. No GCash PIN/password is collected. Real Google routing and real GCash transactions were not exercised in this sandbox. Emulate v0.0.1 was inspected: its Google service does not implement Maps/Routes. The integration test uses its own explicit synthetic Routes HTTP fixture and never configures that fixture as the preview's provider.

## Validation

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
# Includes the MariaDB integration test; TEST_DATABASE_URL database name must end in _test.
cargo test --locked -- --include-ignored
```

`cargo test` alone intentionally skips the database test rather than guessing credentials. Integration fixtures cover a complete sample order, authentication/role and object access, vendor isolation, fee precision, checkout thresholds, immutable prices, concurrent idempotency, manual payment verification, duplicate references, additional-payment consent, substitutions, refunds, chat persistence/WebSockets, tracking termination and reports.

## Deployment, backups and operations

1. Supply a protected production `DATABASE_URL`, exact HTTPS `APP_ORIGIN`, `COOKIE_SECURE=true`, restricted Maps key, and real GCash QR. Build `cargo build --release --locked -p suki-backend`. Keep the database on a private network. Terminate TLS at the ingress and forward `/api/`, `/images/`, and WebSocket upgrades.
2. Apply migrations on a backed-up database before shifting traffic. Startup runs them automatically with SQLx's migration lock. Prefer a dedicated migration user and restricted runtime database permissions when splitting deployment duties.
3. Set an ingress request rate limit for registration/login and API traffic, body limits, WebSocket connection limits, and operational alerting. The built-in identity limits do not replace ingress abuse controls.
4. Monitor `/api/health` (includes database connectivity), process health, route errors, failed payment reviews and backups. Use durable MariaDB storage. Graceful shutdown handles Ctrl-C; the process manager should send SIGINT and wait for in-flight operations.
5. Store a private MariaDB defaults file (`chmod 600`) containing a `[client]` section and backup user credentials. Set `MYSQL_CNF`, `DATABASE_NAME` and run `crates/suki-backend/scripts/backup.sh /private/backups/suki.sql.gz`. Schedule it nightly before accepting real orders, encrypt backups off-host, restrict access, define retention, and alert on failure.
6. Regularly create a separate empty database ending in `_restore`, set `RESTORE_DATABASE`, and run `crates/suki-backend/scripts/restore-test.sh /private/backups/suki.sql.gz`. The script refuses a nonempty target. Compare table counts and important order/payment totals to the source. This restore path was exercised locally against synthetic records; remote scheduling, encryption and recovery objectives remain deployment responsibilities.
7. Schedule `crates/suki-backend/scripts/prune.sql` daily: expires sessions, old attempt counters/quotes, and chat beyond the configured completed/canceled-order retention (default 30 days). Live locations are deleted at completion/cancellation. Agree on legal retention periods and account erasure procedures for financial/order/audit records before production; do not delete financial evidence ad hoc.

The operator must finalize substitution, cancellation, payment dispute and refund policies, perform real-provider acceptance testing, and complete production monitoring/backups/privacy configuration before a controlled pilot. Full administrator/courier dashboards, Google map rendering and courier foreground GPS controls are not yet present in the storefront; their backend endpoints are implemented and tested.
