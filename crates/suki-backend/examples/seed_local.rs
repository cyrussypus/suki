//! Synthetic catalog records for disposable, loopback databases.
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
use std::error::Error;
use suki_api::{id, now};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();
    let options: MySqlConnectOptions = std::env::var("DATABASE_URL")?.parse()?;
    if !matches!(options.get_host(), "localhost" | "127.0.0.1" | "::1")
        || !matches!(options.get_database(), Some("suki" | "suki_test"))
        || std::env::var("SUKI_SEED_LOCAL").as_deref() != Ok("1")
    {
        return Err(
            "Set SUKI_SEED_LOCAL=1 and use a disposable loopback suki or suki_test database".into(),
        );
    }
    let db = MySqlPoolOptions::new().connect_with(options).await?;
    sqlx::migrate!("./migrations").run(&db).await?;
    let mut tx = db.begin().await?;
    let email = "sample-market@suki.invalid";
    let vendor: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE email=?")
        .bind(email)
        .fetch_optional(&mut *tx)
        .await?;
    if vendor.is_some() {
        println!("Sample market already exists.");
        return Ok(());
    }
    // Discard the randomly generated password; no one signs in as this vendor.
    let password = suki_api::auth::password_hash(format!("{}{}", id(), id()))
        .await
        .map_err(|_| "Could not hash the temporary vendor password")?;
    let vendor = id();
    sqlx::query(
        "INSERT INTO users VALUES (?, 'Sample Market', ?, '09000000000', ?, 'vendor', 'active', ?)",
    )
    .bind(&vendor)
    .bind(email)
    .bind(password)
    .bind(now())
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO vendor_profiles VALUES (?, 'Sample Bankerohan Market')")
        .bind(&vendor)
        .execute(&mut *tx)
        .await?;
    for (name, local, category, cents, photo) in [
        ("Fresh Tomatoes", "Kamatis", "Vegetables", 8_000, "tomatoes"),
        ("Benguet Carrots", "Karot", "Vegetables", 10_000, "carrots"),
        ("Fresh Eggplant", "Talong", "Vegetables", 7_500, "eggplant"),
        ("Lacatan Bananas", "Saging", "Fruits", 9_000, "bananas"),
        ("Davao Sweet Mangoes", "Mangga", "Fruits", 16_000, "mangoes"),
        (
            "Fresh Green Cabbage",
            "Repolyo",
            "Vegetables",
            7_000,
            "cabbage",
        ),
        (
            "Fresh Market Fish",
            "Isda",
            "Fish & Seafood",
            22_000,
            "fish",
        ),
        (
            "Fresh Whole Chicken",
            "Manok",
            "Meat & Poultry",
            20_000,
            "chicken",
        ),
    ] {
        sqlx::query("INSERT INTO products(vendor_id,name,local_name,description,category,unit,price_centavos,image,available,updated_at) VALUES (?,?,?,'Sample catalog item',?,'kg',?,?,TRUE,?)")
            .bind(&vendor).bind(name).bind(local).bind(category).bind(cents).bind(format!("/images/{photo}.jpg"))
            .bind(now()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    println!(
        "Seeded a sample vendor and eight products. Routing and payment settings are unchanged."
    );
    Ok(())
}
