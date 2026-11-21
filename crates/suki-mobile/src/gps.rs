use suki_shared::realtime::Position;

#[cfg(target_arch = "wasm32")]
pub struct Source;

#[cfg(target_arch = "wasm32")]
impl Source {
    pub fn new() -> Result<Self, String> {
        Ok(Self)
    }

    pub async fn position(&self) -> Result<Position, String> {
        use std::{cell::RefCell, rc::Rc};
        use wasm_bindgen::{JsCast, closure::Closure};
        let geolocation = web_sys::window()
            .ok_or("Location is unavailable")?
            .navigator()
            .geolocation()
            .map_err(|_| "Location is unavailable")?;
        let options = web_sys::PositionOptions::new();
        options.set_enable_high_accuracy(true);
        options.set_maximum_age(5_000);
        options.set_timeout(10_000);
        let (sender, receiver) = futures_channel::oneshot::channel();
        let sender = Rc::new(RefCell::new(Some(sender)));
        let success = {
            let sender = sender.clone();
            Closure::<dyn FnMut(web_sys::Position)>::new(move |value: web_sys::Position| {
                if let Some(sender) = sender.borrow_mut().take() {
                    let coords = value.coords();
                    let _ = sender.send(Ok(Position {
                        lat: coords.latitude(),
                        lng: coords.longitude(),
                    }));
                }
            })
        };
        let error = Closure::<dyn FnMut(web_sys::PositionError)>::new(
            move |error: web_sys::PositionError| {
                if let Some(sender) = sender.borrow_mut().take() {
                    let _ = sender.send(Err(error.message()));
                }
            },
        );
        let watch = geolocation
            .watch_position_with_error_callback_and_options(
                success.as_ref().unchecked_ref(),
                Some(error.as_ref().unchecked_ref()),
                &options,
            )
            .map_err(|_| "Unable to start location sampling")?;
        struct Watch(web_sys::Geolocation, i32);
        impl Drop for Watch {
            fn drop(&mut self) {
                self.0.clear_watch(self.1);
            }
        }
        let _watch = Watch(geolocation, watch);
        receiver
            .await
            .map_err(|_| "Location request ended unexpectedly")?
    }
}

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use jni::{
        JNIEnv, JavaVM,
        errors::Result as JniResult,
        objects::{GlobalRef, JObject, JValue},
    };

    pub struct Source;

    fn with_activity<T>(
        call: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> JniResult<T>,
    ) -> Result<T, String> {
        let context = ndk_context::android_context();
        // Tao keeps the VM and Activity global reference alive for the Dioxus UI lifetime.
        let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }
            .map_err(|_| "Android VM unavailable")?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|_| "Cannot attach to Android")?;
        let activity = unsafe { JObject::from_raw(context.context().cast()) };
        let result = env.with_local_frame(64, |env| call(env, &activity));
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        result.map_err(|_| {
            "Android could not supply a location. Check permissions and Location settings.".into()
        })
    }

    fn permitted(env: &mut JNIEnv<'_>, activity: &JObject<'_>, name: &str) -> JniResult<bool> {
        let permission = env.new_string(name)?;
        Ok(env
            .call_method(
                activity,
                "checkSelfPermission",
                "(Ljava/lang/String;)I",
                &[JValue::Object(&permission)],
            )?
            .i()?
            == 0)
    }

    const FINE: &str = "android.permission.ACCESS_FINE_LOCATION";
    const COARSE: &str = "android.permission.ACCESS_COARSE_LOCATION";

    struct Pending {
        cancel: GlobalRef,
        executor: GlobalRef,
        builder: GlobalRef,
    }

    impl Drop for Pending {
        fn drop(&mut self) {
            let _ = with_activity(|env, _| {
                env.call_method(self.cancel.as_obj(), "cancel", "()V", &[])?;
                env.call_method(self.executor.as_obj(), "shutdown", "()V", &[])?;
                Ok(())
            });
        }
    }

    impl Source {
        pub fn new() -> Result<Self, String> {
            with_activity(|env, activity| {
                if !permitted(env, activity, COARSE)? && !permitted(env, activity, FINE)? {
                    let values = env.new_object_array(2, "java/lang/String", JObject::null())?;
                    env.set_object_array_element(&values, 0, env.new_string(FINE)?)?;
                    env.set_object_array_element(&values, 1, env.new_string(COARSE)?)?;
                    env.call_method(
                        activity,
                        "requestPermissions",
                        "([Ljava/lang/String;I)V",
                        &[JValue::Object(&values), JValue::Int(211)],
                    )?;
                }
                Ok(())
            })?;
            Ok(Self)
        }

        pub async fn position(&self) -> Result<Position, String> {
            let (sample, pending) = with_activity(|env, activity| {
                if !env
                    .call_method(activity, "hasWindowFocus", "()Z", &[])?
                    .z()?
                {
                    return Ok((None, None));
                }
                let sdk = env
                    .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?
                    .i()?;
                let fine = permitted(env, activity, FINE)?;
                if !fine && !permitted(env, activity, COARSE)? {
                    return Ok((None, None));
                }
                let service = env.new_string("location")?;
                let manager = env
                    .call_method(
                        activity,
                        "getSystemService",
                        "(Ljava/lang/String;)Ljava/lang/Object;",
                        &[JValue::Object(&service)],
                    )?
                    .l()?;
                let provider = env.new_string(if fine { "gps" } else { "network" })?;
                if sdk < 30 {
                    let location = env
                        .call_method(
                            &manager,
                            "getLastKnownLocation",
                            "(Ljava/lang/String;)Landroid/location/Location;",
                            &[JValue::Object(&provider)],
                        )?
                        .l()?;
                    return Ok((read_sample(env, &location)?, None));
                }
                // Stream.Builder implements Consumer, so Android can deliver the single fix
                // through a public Java API. Read the builder only after the executor finishes.
                let builder = env
                    .call_static_method(
                        "java/util/stream/Stream",
                        "builder",
                        "()Ljava/util/stream/Stream$Builder;",
                        &[],
                    )?
                    .l()?;
                let unit = env
                    .get_static_field(
                        "java/util/concurrent/TimeUnit",
                        "SECONDS",
                        "Ljava/util/concurrent/TimeUnit;",
                    )?
                    .l()?;
                let queue =
                    env.new_object("java/util/concurrent/LinkedBlockingQueue", "()V", &[])?;
                let executor = env.new_object(
                    "java/util/concurrent/ThreadPoolExecutor",
                    "(IIJLjava/util/concurrent/TimeUnit;Ljava/util/concurrent/BlockingQueue;)V",
                    &[
                        JValue::Int(1),
                        JValue::Int(1),
                        JValue::Long(0),
                        JValue::Object(&unit),
                        JValue::Object(&queue),
                    ],
                )?;
                let cancel = env.new_object("android/os/CancellationSignal", "()V", &[])?;
                let pending = Pending {
                    builder: env.new_global_ref(&builder)?,
                    executor: env.new_global_ref(&executor)?,
                    cancel: env.new_global_ref(&cancel)?,
                };
                env.call_method(&manager,"getCurrentLocation",
                    "(Ljava/lang/String;Landroid/os/CancellationSignal;Ljava/util/concurrent/Executor;Ljava/util/function/Consumer;)V",
                    &[JValue::Object(&provider), JValue::Object(&cancel), JValue::Object(&executor), JValue::Object(&builder)])?;
                Ok((None, Some(pending)))
            })?;
            if let Some(sample) = sample {
                return Ok(sample);
            }
            if let Some(pending) = pending {
                for _ in 0..30 {
                    crate::api::delay(400).await;
                    let (finished, sample) = with_activity(|env, activity| {
                        if !env
                            .call_method(activity, "hasWindowFocus", "()Z", &[])?
                            .z()?
                        {
                            return Ok((true, None));
                        }
                        let count = env
                            .call_method(
                                pending.executor.as_obj(),
                                "getCompletedTaskCount",
                                "()J",
                                &[],
                            )?
                            .j()?;
                        if count == 0 {
                            return Ok((false, None));
                        }
                        let stream = env
                            .call_method(
                                pending.builder.as_obj(),
                                "build",
                                "()Ljava/util/stream/Stream;",
                                &[],
                            )?
                            .l()?;
                        let iterator = env
                            .call_method(&stream, "iterator", "()Ljava/util/Iterator;", &[])?
                            .l()?;
                        if !env.call_method(&iterator, "hasNext", "()Z", &[])?.z()? {
                            return Ok((true, None));
                        }
                        let location = env
                            .call_method(&iterator, "next", "()Ljava/lang/Object;", &[])?
                            .l()?;
                        Ok((true, read_sample(env, &location)?))
                    })?;
                    if let Some(sample) = sample {
                        return Ok(sample);
                    }
                    if finished {
                        break;
                    }
                }
            }
            Err("Waiting for a fresh Android location. Keep Suki open and grant location permission.".into())
        }
    }

    fn read_sample(env: &mut JNIEnv<'_>, location: &JObject<'_>) -> JniResult<Option<Position>> {
        if location.is_null() {
            return Ok(None);
        }
        let time = env
            .call_method(location, "getElapsedRealtimeNanos", "()J", &[])?
            .j()?;
        let now = env
            .call_static_method("android/os/SystemClock", "elapsedRealtimeNanos", "()J", &[])?
            .j()?;
        if !(0..=15_000_000_000).contains(&now.saturating_sub(time)) {
            return Ok(None);
        }
        Ok(Some(Position {
            lat: env.call_method(location, "getLatitude", "()D", &[])?.d()?,
            lng: env.call_method(location, "getLongitude", "()D", &[])?.d()?,
        }))
    }
}

#[cfg(target_os = "android")]
pub use android::Source;

#[cfg(target_os = "ios")]
mod ios {
    use super::*;
    use objc2::{MainThreadMarker, rc::Retained};
    use objc2_core_location::CLLocationManager;
    use objc2_ui_kit::{UIApplication, UIApplicationState};

    pub struct Source {
        manager: Retained<CLLocationManager>,
        thread: MainThreadMarker,
    }

    impl Source {
        pub fn new() -> Result<Self, String> {
            let thread =
                MainThreadMarker::new().ok_or("Open location sharing from the app screen")?;
            // Core Location is created and used exclusively on the main UI run loop.
            let manager = unsafe { CLLocationManager::new() };
            unsafe {
                manager.requestWhenInUseAuthorization();
                manager.startUpdatingLocation();
            }
            Ok(Self { manager, thread })
        }

        pub async fn position(&self) -> Result<Position, String> {
            if UIApplication::sharedApplication(self.thread).applicationState()
                != UIApplicationState::Active
            {
                return Err("Open Suki to share your location.".into());
            }
            let sample =
                unsafe { self.manager.location() }.ok_or("Waiting for a fresh location")?;
            let age = unsafe { sample.timestamp() }.timeIntervalSinceNow();
            if !(-15.0..=1.0).contains(&age) {
                return Err("Waiting for a fresh iPhone location".into());
            }
            let point = unsafe { sample.coordinate() };
            Ok(Position {
                lat: point.latitude,
                lng: point.longitude,
            })
        }
    }

    impl Drop for Source {
        fn drop(&mut self) {
            unsafe { self.manager.stopUpdatingLocation() }
        }
    }
}

#[cfg(target_os = "ios")]
pub use ios::Source;

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub struct Source;

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
impl Source {
    pub fn new() -> Result<Self, String> {
        Err("Device location is available on mobile and in the browser preview.".into())
    }
    pub async fn position(&self) -> Result<Position, String> {
        unreachable!("GPS source is unavailable")
    }
}
