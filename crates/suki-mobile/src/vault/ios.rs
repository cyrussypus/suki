use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use security_framework::{
    access_control::{ProtectionMode, SecAccessControl},
    passwords::{self, AccessControlOptions, PasswordOptions},
};
use std::sync::Mutex;

const SERVICE: &str = "ph.sukidavao.mobile.session.v1";
const RECORD: &str = "session";
const CONTEXT_BYTES: usize = 37;
const NOT_FOUND: i32 = -25300;
type VaultResult<T> = Result<T, String>;
static MASTER: Mutex<Option<Vec<u8>>> = Mutex::new(None);

fn problem() -> String {
    "Could not unlock secure storage. You can use your Suki password.".into()
}

fn read(name: &str) -> VaultResult<Option<Vec<u8>>> {
    match passwords::get_generic_password(SERVICE, name) {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.code() == NOT_FOUND => Ok(None),
        Err(_) => Err(problem()),
    }
}

fn write(name: &str, data: &[u8], locked: bool) -> VaultResult<()> {
    let mut options = PasswordOptions::new_generic_password(SERVICE, name);
    let flags = if locked {
        AccessControlOptions::USER_PRESENCE
    } else {
        AccessControlOptions::empty()
    };
    let protection = if locked {
        ProtectionMode::AccessibleWhenPasscodeSetThisDeviceOnly
    } else {
        ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly
    };
    options.set_access_control(
        SecAccessControl::create_with_protection(Some(protection), flags.bits())
            .map_err(|_| problem())?,
    );
    passwords::set_generic_password_options(data, options).map_err(|_| problem())
}

fn remove(name: &str) -> VaultResult<()> {
    match passwords::delete_generic_password(SERVICE, name) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == NOT_FOUND => Ok(()),
        Err(_) => Err(problem()),
    }
}

fn key(bytes: &[u8]) -> VaultResult<LessSafeKey> {
    Ok(LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, bytes).map_err(|_| problem())?,
    ))
}

fn seal(master: &[u8], value: &[u8], locked: bool, name: &str) -> VaultResult<Vec<u8>> {
    let mut nonce = [0; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| problem())?;
    let context = [&[u8::from(locked)][..], name.as_bytes()].concat();
    let mut body = value.to_vec();
    key(master)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(&context),
            &mut body,
        )
        .map_err(|_| problem())?;
    Ok([context.as_slice(), &nonce, &body].concat())
}

fn open(master: &[u8], record: &[u8]) -> VaultResult<String> {
    if record.len() < CONTEXT_BYTES + 28 || record[0] > 1 {
        return Err(problem());
    }
    let nonce = Nonce::try_assume_unique_for_key(&record[CONTEXT_BYTES..CONTEXT_BYTES + 12])
        .map_err(|_| problem())?;
    let mut body = record[CONTEXT_BYTES + 12..].to_vec();
    let plaintext = key(master)?
        .open_in_place(nonce, Aad::from(&record[..CONTEXT_BYTES]), &mut body)
        .map_err(|_| problem())?;
    String::from_utf8(plaintext.to_vec()).map_err(|_| problem())
}

fn record_key(record: &[u8]) -> VaultResult<&str> {
    let name = std::str::from_utf8(record.get(1..CONTEXT_BYTES).ok_or_else(problem)?)
        .map_err(|_| problem())?;
    uuid::Uuid::parse_str(name).map_err(|_| problem())?;
    Ok(name)
}

fn perform(operation: &str, value: Option<String>) -> VaultResult<String> {
    let mut master = MASTER.lock().map_err(|_| problem())?;
    let record = read(RECORD)?;
    let locked = record.as_ref().and_then(|r| r.first()).copied() == Some(1);
    let name = record.as_deref().map(record_key).transpose()?;
    let mode = if locked { "1" } else { "0" };
    match operation {
        "enabled" => Ok(mode.into()),
        "clear" => {
            remove(RECORD)?;
            if let Some(mut bytes) = master.take() {
                bytes.fill(0);
            }
            if let Some(name) = name {
                remove(name)?;
            }
            Ok("0".into())
        }
        "load" | "confirm" => {
            let Some(record) = record.as_ref() else {
                return Ok(String::new());
            };
            let mut raw = read(name.ok_or_else(problem)?)?.ok_or_else(problem)?;
            let token = open(&raw, record)?;
            if operation == "confirm" {
                raw.fill(0);
                return Ok(mode.into());
            }
            if let Some(mut previous) = master.replace(raw) {
                previous.fill(0);
            }
            Ok(token)
        }
        "save" | "password_save" if master.is_some() && record.is_some() => {
            let token = value.ok_or_else(problem)?;
            passwords::set_generic_password(
                SERVICE,
                RECORD,
                &seal(
                    master.as_ref().ok_or_else(problem)?,
                    token.as_bytes(),
                    locked,
                    name.ok_or_else(problem)?,
                )?,
            )
            .map_err(|_| problem())?;
            Ok(mode.into())
        }
        "save" | "password_save" | "enable" | "disable" => {
            let next_locked = if operation == "save" {
                locked
            } else {
                operation == "enable"
            };
            let target = uuid::Uuid::new_v4().to_string();
            // A password sign-in is the fallback after an unavailable or cancelled biometric prompt.
            // It can replace an inaccessible vault; the user can re-enable biometric unlock.
            let token = match value {
                Some(token) => token,
                None => {
                    let record = record.as_ref().ok_or_else(problem)?;
                    let raw = if locked {
                        read(name.ok_or_else(problem)?)?.ok_or_else(problem)?
                    } else {
                        master.as_ref().ok_or_else(problem)?.clone()
                    };
                    open(&raw, record)?
                }
            };
            let mut raw = vec![0; 32];
            SystemRandom::new().fill(&mut raw).map_err(|_| problem())?;
            let next_record = seal(&raw, token.as_bytes(), next_locked, &target)?;
            write(&target, &raw, next_locked)?;
            if next_locked && operation == "enable" {
                match read(&target) {
                    Ok(Some(check)) if check == raw => {}
                    _ => {
                        let _ = remove(&target);
                        return Err(problem());
                    }
                }
            }
            let saved = if record.is_some() {
                passwords::set_generic_password(SERVICE, RECORD, &next_record)
                    .map_err(|_| problem())
            } else {
                write(RECORD, &next_record, false)
            };
            if let Err(error) = saved {
                let _ = remove(&target);
                return Err(error);
            }
            if let Some(mut previous) = master.replace(raw) {
                previous.fill(0);
            }
            if let Some(name) = name {
                let _ = remove(name);
            }
            Ok(if next_locked { "1" } else { "0" }.into())
        }
        _ => Err(problem()),
    }
}

pub async fn operate(operation: &str, value: Option<String>) -> VaultResult<String> {
    let (send, receive) = futures_channel::oneshot::channel();
    let operation = operation.to_owned();
    std::thread::Builder::new()
        .spawn(move || {
            let _ = send.send(perform(&operation, value));
        })
        .map_err(|_| problem())?;
    receive.await.map_err(|_| problem())?
}
