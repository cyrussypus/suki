<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:0dbcc2e84c952b9e8c58ce6a2cde8469d2ead2039d7f3f2bab3d279f62c15a8b -->

# Suki

Suki is a Davao City market shopping app. Its Dioxus UI builds for Android and iOS, and the Axum API stores marketplace data in MariaDB 11.8 LTS. The workspace uses Rust 2024 and the Rust 1.99.1 pin in `rust-toolchain.toml`.

| Crate | Responsibility |
| --- | --- |
| [suki-mobile](crates/suki-mobile) | Dioxus screens, accounts, shopping, GCash references, chat and tracking |
| [suki-backend](crates/suki-backend) | Axum API, SQLx migrations, authorization and order transactions; the binary is named `suki-api` |
| [suki-shared](crates/suki-shared) | API models, events and exact centavo calculations |

See [Setup](docs/SETUP.md) for development and native builds and [API and operations](crates/suki-backend/README.md) for endpoints and backup procedures.

```sh
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --locked
# With TEST_DATABASE_URL pointing to a disposable MariaDB 11.8 database ending in _test:
cargo test --locked -- --include-ignored
```

The mobile app includes customer shopping and order management, vendor stock controls, and assigned courier location reporting. Admin operations are available through the API. Checkout requires a configured Google Routes key and the operator’s actual GCash QR. Mobile sessions, favorites and baskets are held in memory while the app runs.


<!-- /coderabbit-ai-generated:v1 -->