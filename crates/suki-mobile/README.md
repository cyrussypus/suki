# Suki Mobile

See [Setup](../../docs/SETUP.md#native-mobile-builds) for the Android and iOS commands and how to set the API origin. Run builds with Dioxus CLI 0.7.10. Android requires API 28 or newer. The build script preserves 16 KB page alignment on Android.

<!-- coderabbit-ai-generated:v1 -->

<!-- coderabbit-ai-generated-section:v1:891704939d80349a553ddd774220b0a58c1ce5de17dd850723d350e46063134c -->

The app keeps its access credential, basket, and favorites in memory. Android Keystore and iOS Keychain protect the remembered refresh credential. The web preview uses same-origin HttpOnly cookies. The server owns payments, order authorization, prices, and routing.

<!-- /coderabbit-ai-generated:v1 -->

The optional browser preview reuses the Rust Dioxus screens for development. Use native device testing for platform behavior.


