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

The API applies migrations before listening. Check `curl --fail http://localhost:8080/api/health`. From the workspace root use `cargo run --locked -p suki-backend` with variables supplied by the shell or your process manager. The sample environment describes administrator bootstrap. Remove bootstrap credentials after first startup.

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