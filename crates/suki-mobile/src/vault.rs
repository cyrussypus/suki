#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

pub const NATIVE: bool = cfg!(any(target_os = "android", target_os = "ios"));

pub async fn operate(operation: &str, value: Option<String>) -> Result<String, String> {
    #[cfg(target_os = "android")]
    return android::operate(operation, value).await;
    #[cfg(target_os = "ios")]
    return ios::operate(operation, value).await;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (operation, value);
        Ok(String::new())
    }
}
