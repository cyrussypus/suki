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

For disposable local data, run `SUKI_SEED_LOCAL=1 cargo run --locked -p suki-backend --example seed_local` with the local `DATABASE_URL` set. The seed adds a synthetic vendor and products.

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

Set `GOOGLE_MAPS_API_KEY` on the API. The server verifies driving distance and enforces service coverage and minimum goods subtotal. Mobile checkout accepts an address and coordinates or an existing saved address. The assigned courier can report coordinates during active delivery.

Serve the operator’s actual GCash QR in `SUKI_IMAGE_DIR` or the backend’s `assets/images`, and set its `/images/...` URL in the administrator settings API. The customer submits a reference for the outstanding amount. The administrator verifies the actual transfer before shopping can start.

See [Backend operations](../crates/suki-backend/README.md#deployment-backups-and-operations) and [Architecture](ARCHITECTURE.md).


<!-- /coderabbit-ai-generated:v1 -->