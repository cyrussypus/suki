<!-- coderabbit-ai-generated:v1 -->

# System architecture

Dioxus components own screen state through signals. The Rust transport uses an in-memory cookie jar on native targets and same-origin browser cookies for development. Native WebSocket requests forward the session cookie and configured origin. The API validates both and periodically rechecks order access. Navigating away stops the subscription. On reconnect the client recovers persisted messages by cursor and refreshes the order and courier location.

Axum handlers own authorization and transactions. SQLx connects to MariaDB 11.8 LTS using its MySQL driver. Existing migration names and contents are unchanged so an existing database can keep its migration history. The separate PostgreSQL foundation prototype was removed from the product tree.

The shared crate owns wire types and exact centavo arithmetic. The API revalidates product availability and expected prices, the customer-owned quote and the minimum goods subtotal within checkout. Retries preserve the same payload and idempotency key when the outcome is uncertain. A payment reference starts manual review.
