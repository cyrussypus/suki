# Suki Mobile

See [Setup](../../docs/SETUP.md#native-mobile-builds) for the Android and iOS commands and how to set the API origin. Run builds with Dioxus CLI 0.7.10. Android requires API 28 or newer. The build script preserves 16 KB page alignment on Android.

The app keeps its session cookie, basket, and favorites in memory. Login uses the same Axum endpoint as the preview. The server owns payment verification, order authorization, prices and road distance.

The optional browser preview reuses the Rust Dioxus screens for development. Use native device testing for platform behavior.


