use crate::{Api, Page, State};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use reqwest::Method;
use suki_shared::{
    Acknowledged,
    auth::{Account, Login, Registration, RegistrationResult, Role},
};

#[component]
pub fn AccountScreen() -> Element {
    let api = use_context::<Api>();
    let mut state = use_context::<State>();
    let mut busy = use_signal(|| false);
    let mut register = use_signal(|| false);
    let mut vendor_signup = use_signal(|| false);
    let mut email = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut name = use_signal(String::new);
    let mut phone = use_signal(String::new);
    let mut shop = use_signal(String::new);
    let mut error = use_signal(String::new);
    if let Some(account) = (state.account)() {
        return rsx! {
            section { class: "account-page",
                p { class: "eyebrow", "GOOD TO SEE YOU" }
                h1 { "Your account" }
                div { class: "profile-card",
                    span { class: "profile-avatar", "✳" }
                    div { h2 { "{account.name}" } p { "{account.email}" } small { "{account.role}" } }
                }
                div { class: "account-menu",
                    if account.has_role(Role::Customer) {
                        button { onclick: move |_| { state.active_order.set(None); state.page.set(Page::Orders); }, span { class: "menu-icon", "▤" } span { strong { "My orders" } small { "Follow your market finds" } } b { "›" } }
                        button { onclick: move |_| state.page.set(Page::Favorites), span { class: "menu-icon", "♡" } span { strong { "Saved favorites" } small { "Find the things you love" } } b { "›" } }
                        button { onclick: move |_| state.page.set(Page::Basket), span { class: "menu-icon", "▱" } span { strong { "Your basket" } small { "Review your fresh picks" } } b { "›" } }
                    } else {
                        button { onclick: move |_| state.page.set(Page::Dashboard), span { class: "menu-icon", "▦" } span { strong { "Dashboard" } small { "View your workspace" } } b { "›" } }
                    }
                }
                if !(error)().is_empty() { p { role: "alert", "{error}" } }
                button { class: "signout", disabled: (state.signing_out)(), onclick: move |_| {
                    if (state.signing_out)() { return; }
                    let api = api.clone();
                    state.signing_out.set(true);
                    spawn_forever(async move {
                        let result = api.send::<Acknowledged, _>(Method::POST, "/auth/logout", &()).await;
                        let revoked = match result {
                            Ok(_) => true,
                            Err(failure) => {
                                match api.get::<Account>("/auth/me").await {
                                    Err(probe) if probe.status == Some(401) => true,
                                    _ => {
                                        state.notice.set(failure.to_string());
                                        false
                                    }
                                }
                            }
                        };
                        if revoked {
                            state.account.set(None);
                            state.basket.write().clear();
                            state.favorites.write().clear();
                            state.active_order.set(None);
                            state.pending.set(None);
                            state.page.set(Page::Market);
                            state.notice.set("Signed out.".into());
                        }
                        state.signing_out.set(false);
                    });
                }, "Sign out" }
            }
        };
    }
    rsx! {
        section { class: "panel auth-panel", p { class: "eyebrow", "WELCOME TO SUKI" } h1 { if register() { "Create a Suki account" } else { "Sign in to Suki" } }
            form {
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy() { return; }
                    let api = api.clone();
                    let login = Login { email: email(), password: password() };
                    let registration = register().then(|| Registration {
                        name: name(), email: login.email.clone(), phone: phone(),
                        password: login.password.clone(),
                        role: if vendor_signup() { Role::Vendor } else { Role::Customer }.as_str().into(),
                        shop_name: vendor_signup().then(&*shop),
                    });
                    busy.set(true);
                    error.set(String::new());
                    spawn(async move {
                        let result = async {
                            if let Some(registration) = registration {
                                let result: RegistrationResult = api.send(Method::POST, "/auth/register", &registration).await?;
                                if result.status == "pending" {
                                    register.set(false);
                                    password.set(String::new());
                                    state.notice.set("Your vendor account is awaiting administrator approval.".into());
                                    return Ok::<_, crate::api::Failure>(());
                                }
                                register.set(false);
                            }
                            api.send::<Acknowledged, _>(Method::POST, "/auth/login", &login).await?;
                            let account: Account = api.get("/auth/me").await?;
                            state.notice.set(format!("Welcome, {}.", account.name));
                            state.page.set(crate::home_for(&account));
                            state.account.set(Some(account));
                            password.set(String::new());
                            Ok(())
                        }.await;
                        if let Err(failure) = result { error.set(failure.to_string()); }
                        busy.set(false);
                    });
                },
                fieldset { disabled: busy(),
                    if register() {
                        label { "Name" input { required: true, autocomplete: "name", value: "{name}", oninput: move |e| name.set(e.value()) } }
                        label { "Phone" input { required: true, r#type: "tel", autocomplete: "tel", value: "{phone}", oninput: move |e| phone.set(e.value()) } }
                        label { class: "check", input { r#type: "checkbox", checked: vendor_signup(), onchange: move |e| vendor_signup.set(e.checked()) } "Register as a vendor" }
                        if vendor_signup() {
                            label { "Shop name" input { required: true, value: "{shop}", oninput: move |e| shop.set(e.value()) } }
                        }
                    }
                    label { "Email" input { required: true, r#type: "email", autocomplete: "email", value: "{email}", oninput: move |e| email.set(e.value()) } }
                    label { "Password" input {
                        required: true, r#type: "password",
                        autocomplete: if register() { "new-password" } else { "current-password" },
                        minlength: if register() { "12" }, maxlength: "128",
                        value: "{password}", oninput: move |e| password.set(e.value())
                    } }
                    if !(error)().is_empty() { p { role: "alert", "{error}" } }
                    button { class: "primary", r#type: "submit",
                        if busy() { "Please wait…" } else if register() { "Create account" } else { "Sign in" }
                    }
                    button { class: "quiet", r#type: "button", onclick: move |_| { register.toggle(); error.set(String::new()); },
                        if register() { "Already have an account?" } else { "Create an account" }
                    }
                }
            }
        }
    }
}
