# Suki API

See the [setup guide](../../docs/SETUP.md) for database creation and native app builds. To start the API, copy `.env.example` to `.env` in this directory, set private environment values, and run `cargo run --locked`.

Startup applies the versioned migrations before listening. The bootstrap administrator is created only when no administrator exists. New vendors remain pending until activated. Public registration cannot create administrators or couriers. An administrator may promote an existing account to courier or be assigned to an order.

The API binary is `suki-api`. Its image directory can be configured with `SUKI_IMAGE_DIR`. The optional development preview and the native client use the same API endpoints.



## Security and storage

Admin privileges require an active administrator account and an email in the server's comma-separated `ADMIN_EMAILS`. Matching ignores surrounding whitespace and letter case. Set the variable before upgrading: an absent or empty list disables admin access. An unlisted administrator receives HTTP 403 at sign-in and on existing authenticated sessions. An allowlisted email does not grant the admin role. The list is loaded at API startup, so restart each instance after editing it. Keep the list when removing the one-time bootstrap credentials, and include the bootstrap email when creating the first admin.

The client uses the server's `admin_access` flag from `GET /api/auth/me` to show the admin dashboard. The API independently enforces both checks before allowing admin operations.


- Argon2id password hashes; bounded password workers; per-identity authentication attempt limits.


- Access JWTs expire after 15 minutes and use the independent `SESSION_SIGNING_KEY`. Each device session lasts up to 30 days and has a rotating opaque refresh credential stored only as a SHA-256 hash. Cookies are HttpOnly, SameSite=Lax and Secure outside localhost development. Requests check the MariaDB session and current account state. Logout, remote revocation and administrative account changes invalidate device sessions.

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
| POST | `/auth/login` | `{email,password}`; sets access and refresh cookies |
| POST | `/auth/refresh` | Rotates the refresh cookie and issues a new access cookie |
| GET | `/auth/sessions` | Lists the signed-in account’s active device sessions |
| DELETE | `/auth/sessions/{id}` | Revokes an active device session owned by the signed-in account |
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



The owning vendor can change stock availability with `PATCH /api/products/{id}/availability` and `{available}`. This leaves other product fields unchanged.


Principal flow: `awaiting_payment → payment_verification → paid → shopping → ready_for_delivery → out_for_delivery → delivered → completed`. A higher spending authorization pauses shopping in `price_adjustment`; customer approval and verified additional payment resume it. Reconciliation sets `ready_for_delivery`; clients cannot jump directly there. Customers may cancel before shopping; administrators may cancel before delivery completion, creating the remaining refund obligation. Both must wait until pending payment evidence is verified or rejected before canceling.

## Google Maps and GCash

Configure a restricted server `GOOGLE_MAPS_API_KEY` with Google Routes `directions/v2:computeRoutes`. The API sends driving origin/destination coordinates and requests `routes.distanceMeters`. It validates coordinate ranges, configured bounding-box coverage and maximum road distance. The default origin is Bankerohan. Administrators must review the origin and coverage against actual operations before launch.



Marketplace endpoints (prefix `/api`):

| Method | Path | Purpose |
| --- | --- | --- |
| POST, PUT | `/vendor/goods`, `/vendor/goods/{id}` | Create or edit an owned product. Send multipart `product` JSON plus a JPEG or PNG `photo` (optional when editing). |
| PATCH | `/vendor/goods/{id}/price`, `/vendor/goods/{id}/availability` | Update only `price_centavos` or `available`, respectively. |
| GET, PUT | `/vendor/payment-profile` | Read or change the signed-in vendor's `gcash_number`. |
| POST | `/quotes/delivery` | Quote using `address`, `lat`, `lng`, and `weight_grams`. |
| GET | `/orders/{id}/chat/ws` | Customer, order vendor, and assigned rider receive committed chat events. Use the existing messages path to post and catch up. |
| POST | `/orders/{id}/cash-collection` | Assigned rider attests to the exact reconciled amount collected at handoff. |
| POST | `/orders/{id}/vendor-settlement` | Assigned rider records the receipt or confirms the vendor received the direct transfer. |
| GET | `/admin/payments/pending` | List references awaiting manual verification. |

The new quote uses `max(8500, round((700 × meters + 500 × grams) / 1000))`. Customer checkout supplies `payment_method` (`gcash` or `cash_on_delivery`). Prices and weight are verified again before the order is placed. The legacy quote endpoint is disabled by default; `SUKI_ALLOW_LEGACY_QUOTES=true` temporarily enables it for older deployments. Existing orders use their snapshotted amounts and payment rules.



The backend accepts JPEG, PNG and WebP images up to 5 MiB, validates and re-encodes them, then uploads JPEG objects to Cloudflare R2. Configure all five `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, `R2_BUCKET_NAME` and `R2_PUBLIC_DOMAIN` variables; the public domain must be an HTTPS origin. Missing R2 settings make new uploads unavailable. `R2_ENDPOINT_URL` optionally selects a test endpoint; plain HTTP is accepted only for loopback. Credentials stay on the server.



`POST /api/upload/avatar` updates the authenticated user's avatar; `POST /api/upload/products/{id}` is restricted to the owning vendor. Existing vendor goods forms use R2 too. Both accept multipart `photo`. The server stores object metadata, ownership and R2 URLs in MariaDB and retains old objects so historical same-origin image paths continue to work. New product responses include `image_url`; the current mobile client uses this HTTPS R2 URL directly and falls back to `image` for existing photos. Existing `SUKI_MEDIA_DIR` images remain readable and must still be backed up. R2 uploads and SQL commits are separate operations: a failed database write can leave an unreferenced object, which operators should reconcile before deleting. The configured public domain must resolve to the bucket for direct photo access.


Authentication routes allow five requests per minute per socket IP; uploads allow ten requests per minute per authenticated user; courier location updates allow 120 requests per minute per session. The API ignores forwarded IP headers, so configure an ingress limit per client when behind a proxy. Limits are held in each API process and reset on restart; distributed deployments require shared ingress enforcement.


Real Google routing and GCash transactions need provider testing. The database integration test uses its own synthetic Routes server.


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



Before a controlled pilot, finalize substitution, cancellation, payment dispute, and refund procedures. Verify real Google Routes and GCash transactions, test location sharing on devices, and configure monitoring, backups, and privacy controls.

