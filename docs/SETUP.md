<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:8f735b6ba6bb76f0b2149cd4071bc36451c6d9cfa0ab59ddfa86a634a06b0b8c -->

# Suki setup

## API and MariaDB

Install Rust through [rustup](https://rustup.rs/). Use MariaDB 11.8 LTS. Create UTF-8 databases and a private local user:

```sql
CREATE DATABASE suki CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
CREATE DATABASE suki_test CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
CREATE USER 'suki'@'127.0.0.1' IDENTIFIED BY 'REPLACE_WITH_PRIVATE_PASSWORD';
GRANT ALL PRIVILEGES ON suki.* TO 'suki'@'127.0.0.1';
GRANT ALL PRIVILEGES ON suki_test.* TO 'suki'@'127.0.0.1';
```

```sh
cd crates/suki-backend
cp .env.example .env
chmod 600 .env
# Edit .env with your private database settings.
cargo run --locked
```

<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:5f841eed5076108ed2c406ec7f6b23bcc11d6fc286244cdb0ed794a55b54d8f9 -->

The API applies migrations before listening. Check `curl --fail http://localhost:8080/api/health`. From the workspace root use `cargo run --locked -p suki-backend` with variables supplied by the shell or your process manager. The sample environment describes administrator bootstrap. Remove bootstrap credentials after first startup.

<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:c4d159a8c9d910e71ce26aa9fd0059f154aea0ce299776e5e1feb85ab5e2340e -->

Set `SUKI_INTEGRITY_KEY` before starting or upgrading the API. Generate it once with `openssl rand -hex 32`, keep it in your server secret store and back it up separately from MariaDB. All API instances must use the same key. Replacing it without a migration makes existing signatures fail verification. The migration requires permission to create triggers. Apply schema upgrades with older API instances stopped.

**Recover an interrupted integrity migration.** MariaDB commits schema changes separately, so startup can leave part of migration 0004 or 0005 applied. Before upgrading, stop all API instances and other writers and take a validated database backup. Keep writes stopped until the migrated API passes its health check.

If startup fails during migration, stop retries and keep the failed database for inspection. Check the migration records and which schema objects already exist:

```sql
SELECT version, success FROM _sqlx_migrations WHERE version IN (4, 5);
SHOW TABLES LIKE 'order_integrity';
SHOW TABLES LIKE 'order_snapshots';
SHOW COLUMNS FROM payments LIKE 'reference_hash';
SHOW COLUMNS FROM chat_messages LIKE 'signature';
SHOW TRIGGERS;
```

Restore the pre-upgrade backup into a fresh database, verify the restored migration records, and run the upgrade there with the original signing key. Switch the API to that database only after the upgrade and health checks pass. Keep the failed database unchanged; if anything wrote to it after the backup, reconcile those writes before switching. Do not mark a partial migration successful or drop tables from the failed database to force a retry.

<!-- /coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:cb42e4360e03761a12e8e029a3d2625c40a225ae8226f45c8bac2f8d01349a82 -->

Use **Order integrity inspector** on the admin dashboard to inspect a full order ID. The server verifies the transaction chain and stored chat signatures. An unsigned transaction entry indicates a write outside the API and blocks further changes through the API until investigated. Orders and chat created before the migration have no historical proof. The chain detects modification of signed entries and gaps before later entries; a privileged database restore or truncation requires an independent backup or checkpoint to detect. GCash fingerprints reject reuse of a normalized reference. Staff still confirm the transfer directly, as a fingerprint cannot prove receipt.

New checkouts save one signed receipt in `order_snapshots` as part of the order transaction. The receipt locks the listing, vendor, weight, checkout price, payment method and quoted delivery charges. The customer and assigned rider can see this receipt; vendors can see only their own lines. The admin inspector checks the receipt against its signed audit entry. Orders placed before signed receipts were introduced retain their original checkout figures without a signature. The receipt always shows the checkout total. COD collection and vendor settlement continue to use the reconciled amounts displayed separately in the order.

<!-- /coderabbit-ai-generated:v1 -->

<!-- /coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:288da9c549abacfcb9aef48d640a4c9e997f1b06e92f15e32e9af6582ba5663f -->

The catalog starts empty. Register a vendor account, activate it in the admin dashboard, then sign in as the vendor and upload a product. Set `SUKI_MEDIA_DIR` to a writable, persistent directory and include it in backups.

<!-- /coderabbit-ai-generated:v1 -->

## Native mobile builds

<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:65f0c168fb6f736320497b04a139360aa5ae1740adfb2536a307e49d7cb53e6f -->

Run the following commands from the workspace root. Use Dioxus CLI 0.7.10 to match the application. Android requires JDK 17, SDK platform 35, build-tools 35.0.0 and NDK 27.2.12479018; set `ANDROID_HOME` and `NDK_HOME`. iOS requires macOS and Xcode. Device builds need your signing configuration.

<!-- /coderabbit-ai-generated:v1 -->

```sh
cargo install dioxus-cli --version 0.7.10 --locked
rustup target add aarch64-linux-android
export SUKI_API_URL=https://api.your-suki-host.example
cd crates/suki-mobile
dx build --platform android --target aarch64-linux-android --features mobile --locked
# On macOS, for an Apple Silicon simulator:
rustup target add aarch64-apple-ios-sim
dx build --platform ios --target aarch64-apple-ios-sim --features mobile --locked
```

For native builds, `SUKI_API_URL` is the API origin without a path. Set `SUKI_APP_ORIGIN` at build time only if the server uses a different `APP_ORIGIN` for WebSocket validation. Use HTTPS and secure cookies for a device. Device localhost refers to the device itself.

## Browser development preview

<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:25bc8dccfb669f70c3f4e1d8b0863c41ad48c08c89ce7011f896fcd65b9132ec -->

The same Rust UI can compile to WebAssembly for previewing the flows from a browser. Stop the API started above before launching the preview server. Run the following commands from the workspace root, with the API configured for `http://localhost:8080`:

<!-- /coderabbit-ai-generated:v1 -->

```sh
rustup target add wasm32-unknown-unknown
cd crates/suki-mobile
dx build --platform web --features web --locked
cd ../..
SUKI_PREVIEW_DIR="$PWD/target/dx/suki-mobile/debug/web/public" cargo run --locked -p suki-backend
```

Open `http://localhost:8080`. The API serves the generated browser build, images and WebSocket endpoint together so cookie and origin rules still apply. Generated build files are ignored.

## Delivery, payments and operations

<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:149c7c8ca4c59bce806582c1139fe9c57d8fb418964dc1f4894206cdd3afc079 -->

Set `GOOGLE_MAPS_API_KEY` on the API. New quotes use Google Routes driving distance from Bankerohan Market and the estimated basket weight: ₱85 minimum, ₱7/km, plus ₱5/kg. Checkout confirms that all items belong to one stall and revalidates the weight against the current product records. The existing goods minimum still applies.

Vendors configure a GCash mobile number in their dashboard. Customers transfer the order total to that number and submit a reference for manual review by the assigned rider or an administrator. For COD, the rider records the exact cash collected, including delivery, at handoff. The rider records the vendor settlement after delivery. The app records payments and settlements; it does not move money. The operator and vendor must agree how to remit the delivery portion and handle refunds for direct transfers.

Riders can enable location sharing on the active delivery screen. Leaving that screen stops updates. Android 11 and newer request fresh fixes; Android 9–10 can report fresh cached fixes or manual coordinates. iOS uses foreground Core Location. This directory does not include a production payment provider integration.

<!-- /coderabbit-ai-generated:v1 -->

See [Backend operations](../crates/suki-backend/README.md#deployment-backups-and-operations) and [Architecture](ARCHITECTURE.md).


<!-- /coderabbit-ai-generated:v1 -->