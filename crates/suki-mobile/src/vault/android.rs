use jni::{
    JNIEnv, JavaVM,
    errors::Result as JniResult,
    objects::{JClass, JObject, JString, JValue},
};

fn call<T>(
    f: impl FnOnce(&mut JNIEnv<'_>, &JClass<'_>, &JObject<'_>) -> JniResult<T>,
) -> Result<T, String> {
    let context = ndk_context::android_context();
    let vm =
        unsafe { JavaVM::from_raw(context.vm().cast()) }.map_err(|_| "Android VM unavailable")?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|_| "Android storage unavailable")?;
    let activity = unsafe { JObject::from_raw(context.context().cast()) };
    let package = option_env!("SUKI_ANDROID_PACKAGE").unwrap_or("dev.dioxus.main");
    let result = env.with_local_frame(16, |env| {
        let loader = env
            .call_method(
                &activity,
                "getClassLoader",
                "()Ljava/lang/ClassLoader;",
                &[],
            )?
            .l()?;
        let name = env.new_string(format!("{package}.SessionVault"))?;
        let class = env
            .call_method(
                loader,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        f(env, &JClass::from(class), &activity)
    });
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result.map_err(|_| "Could not access Android secure storage".into())
}

struct Ticket(i32);
impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = call(|env, class, _| {
            env.call_static_method(class, "cancel", "(I)V", &[JValue::Int(self.0)])
                .map(|_| ())
        });
    }
}

pub async fn operate(operation: &str, value: Option<String>) -> Result<String, String> {
    let ticket = Ticket(call(|env, class, activity| {
        let op = env.new_string(operation)?;
        let value = match value.as_ref() {
            Some(value) => JObject::from(env.new_string(value)?),
            None => JObject::null(),
        };
        env.call_static_method(
            class,
            "begin",
            "(Landroid/app/Activity;Ljava/lang/String;Ljava/lang/String;)I",
            &[
                JValue::Object(activity),
                JValue::Object(&op),
                JValue::Object(&value),
            ],
        )?
        .i()
    })?);
    for _ in 0..600 {
        let result = call(|env, class, _| {
            let value = env
                .call_static_method(
                    class,
                    "take",
                    "(I)Ljava/lang/String;",
                    &[JValue::Int(ticket.0)],
                )?
                .l()?;
            if value.is_null() {
                return Ok(None);
            }
            let value = JString::from(value);
            Ok(Some(String::from(env.get_string(&value)?)))
        })?;
        if let Some(result) = result {
            return result
                .strip_prefix('!')
                .map_or_else(|| Ok(result.clone()), |message| Err(message.into()));
        }
        crate::api::delay(100).await;
    }
    Err("Secure storage timed out. Try again or sign in with your password.".into())
}
