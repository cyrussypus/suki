use crate::{auth::User, orders, *};
use axum::extract::{Path, State};
use suki_shared::reviews::{OrderReviews, Reputation, Review, ReviewInput};

fn validate(input: &ReviewInput, kind: &str) -> Result<()> {
    if !(1..=5).contains(&input.rating)
        || input
            .comment
            .as_ref()
            .is_some_and(|c| c.len() > 1000 || c.contains('\0'))
        || input.tags.len() > 3
    {
        return Err(ApiError::bad("Invalid rating, comment or tags"));
    }
    let allowed = match kind {
        "vendor" => ["quality", "packaging", "accuracy"],
        "rider" => ["speed", "handling", "courtesy"],
        _ => return Err(ApiError::bad("Invalid review type")),
    };
    if input
        .tags
        .iter()
        .any(|tag| !allowed.contains(&tag.as_str()))
        || input
            .tags
            .iter()
            .enumerate()
            .any(|(i, tag)| input.tags[..i].contains(tag))
    {
        return Err(ApiError::bad("Invalid review tags"));
    }
    Ok(())
}

pub async fn submit(
    State(s): State<App>,
    u: User,
    Path((oid, kind)): Path<(String, String)>,
    Json(input): Json<ReviewInput>,
) -> Result<Json<Acknowledged>> {
    u.require(&["customer"])?;
    validate(&input, &kind)?;
    let mut tx = s.db.begin().await?;
    let order = orders::locked(&mut tx, &u, &oid).await?;
    if !matches!(order.status.as_str(), "delivered" | "completed") || order.delivered_at.is_none() {
        return Err(ApiError::conflict("Reviews are available after delivery"));
    }
    let target = if kind == "vendor" {
        order.vendor_id.as_deref()
    } else {
        order.courier_id.as_deref()
    }
    .ok_or(ApiError::conflict("This order has no eligible recipient"))?;
    if target == u.id {
        return Err(ApiError::forbidden());
    }
    let comment = input
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let tags = serde_json::to_string(&input.tags).expect("serializable tags");
    let review_id = id();
    // Lock the recipient to serialize aggregate updates across different orders.
    if kind == "vendor" {
        let streak: u32 = sqlx::query_scalar(
            "SELECT five_star_streak FROM vendor_profiles WHERE user_id=? FOR UPDATE",
        )
        .bind(target)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO vendor_reviews(id,order_id,customer_id,vendor_id,rating,comment,tags,created_at) VALUES (?,?,?,?,?,?,?,?)").bind(&review_id).bind(&order.id).bind(&u.id).bind(target).bind(input.rating).bind(comment).bind(&tags).bind(now()).execute(&mut *tx).await?;
        let bonus = points(input.rating, streak);
        sqlx::query("UPDATE vendor_profiles SET reputation_score=(SELECT ROUND(AVG(rating),2) FROM vendor_reviews WHERE vendor_id=?), total_reviews=total_reviews+1, reputation_points=reputation_points+?, five_star_streak=IF(?=5, five_star_streak+1, 0) WHERE user_id=?").bind(target).bind(bonus).bind(input.rating).bind(target).execute(&mut *tx).await?;
    } else {
        let streak: u32 =
            sqlx::query_scalar("SELECT five_star_streak FROM users WHERE id=? FOR UPDATE")
                .bind(target)
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query("INSERT INTO rider_reviews(id,order_id,customer_id,rider_id,rating,comment,tags,created_at) VALUES (?,?,?,?,?,?,?,?)").bind(&review_id).bind(&order.id).bind(&u.id).bind(target).bind(input.rating).bind(comment).bind(&tags).bind(now()).execute(&mut *tx).await?;
        let bonus = points(input.rating, streak);
        sqlx::query("UPDATE users SET reputation_score=(SELECT ROUND(AVG(rating),2) FROM rider_reviews WHERE rider_id=?), reputation_points=reputation_points+?, five_star_streak=IF(?=5, five_star_streak+1, 0) WHERE id=?").bind(target).bind(bonus).bind(input.rating).bind(target).execute(&mut *tx).await?;
    }
    audit(
        &mut tx,
        &u.id,
        "review.submit",
        &review_id,
        json!({"order_id":order.id,"kind":kind,"rating":input.rating}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &order.id, &u.id).await?;
    Ok(Json(Acknowledged { ok: true }))
}

fn points(rating: u8, streak: u32) -> i64 {
    match rating {
        5 => 15 + if streak >= 2 { 5 } else { 0 },
        4 => 10,
        _ => 0,
    }
}

pub async fn list(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
) -> Result<Json<OrderReviews>> {
    u.require(&["customer"])?;
    let order = orders::access(&s, &u, &oid).await?;
    async fn find(db: &MySqlPool, sql: &'static str, oid: &str) -> Result<Option<Review>> {
        let row: Option<(u8, Option<String>, String, i64)> =
            sqlx::query_as(sql).bind(oid).fetch_optional(db).await?;
        row.map(|(rating, comment, tags, created_at)| {
            Ok(Review {
                rating,
                comment,
                tags: serde_json::from_str(&tags)
                    .map_err(|_| ApiError::conflict("Invalid review data"))?,
                created_at,
            })
        })
        .transpose()
    }
    Ok(Json(OrderReviews {
        vendor: find(
            &s.db,
            "SELECT rating,comment,tags,created_at FROM vendor_reviews WHERE order_id=?",
            &order.id,
        )
        .await?,
        rider: find(
            &s.db,
            "SELECT rating,comment,tags,created_at FROM rider_reviews WHERE order_id=?",
            &order.id,
        )
        .await?,
    }))
}

pub async fn reputation(
    State(s): State<App>,
    u: User,
    Path((kind, target)): Path<(String, String)>,
) -> Result<Json<Reputation>> {
    // Reputation is only available to active signed-in users.
    let _ = u;
    let (score, total_reviews, reputation_points, total_deliveries): (String, i64, i64, Option<i64>) = match kind.as_str() {
        "vendor" => sqlx::query_as("SELECT CAST(reputation_score AS CHAR),CAST(total_reviews AS SIGNED),CAST(reputation_points AS SIGNED),NULL FROM vendor_profiles WHERE user_id=?").bind(target).fetch_optional(&s.db).await?,
        "rider" => sqlx::query_as("SELECT CAST(reputation_score AS CHAR),CAST((SELECT COUNT(*) FROM rider_reviews WHERE rider_id=users.id) AS SIGNED),CAST(reputation_points AS SIGNED),CAST(total_deliveries AS SIGNED) FROM users WHERE id=? AND role IN ('courier','admin')").bind(target).fetch_optional(&s.db).await?,
        _ => return Err(ApiError::bad("Invalid reputation type")),
    }.ok_or(ApiError(StatusCode::NOT_FOUND, "Recipient not found".into()))?;
    Ok(Json(Reputation {
        score,
        total_reviews,
        reputation_points,
        total_deliveries,
    }))
}

#[cfg(test)]
mod tests {
    use super::points;
    #[test]
    fn awards_review_points_and_consistency_bonus() {
        assert_eq!(points(5, 0), 15);
        assert_eq!(points(5, 2), 20);
        assert_eq!(points(4, 5), 10);
        assert_eq!(points(1, 5), 0);
    }
}
