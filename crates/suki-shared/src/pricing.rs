//! One rounding rule for UI estimates and authoritative MariaDB order snapshots.

pub const DEFAULT_MINIMUM_CENTAVOS: i64 = 45_000;
pub const MAX_GOODS_CENTAVOS: i64 = 100_000_000;
pub const MAX_QUANTITY_MILLI: i64 = 1_000_000;

pub fn cost(price: i64, quantity: i64) -> Option<i64> {
    if !(0..=MAX_GOODS_CENTAVOS).contains(&price) || !(0..=MAX_QUANTITY_MILLI).contains(&quantity) {
        return None;
    }
    Some((price * quantity + 500) / 1000)
}

/// Arguments are bounded by the server's validated settings and verified road distance.
pub fn delivery_fee(base: i64, per_km: i64, meters: i64) -> i64 {
    base + (per_km * meters + 500) / 1000
}

pub fn permits_goods_subtotal(goods: i64, minimum: i64) -> bool {
    (minimum..=MAX_GOODS_CENTAVOS).contains(&goods)
}

pub fn peso(centavos: i64) -> String {
    let abs = centavos.unsigned_abs();
    format!(
        "{}₱{}.{:02}",
        if centavos < 0 { "-" } else { "" },
        abs / 100,
        abs % 100
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_round_half_up_without_floating_point() {
        assert_eq!(cost(101, 500), Some(51));
        assert_eq!(cost(1, 499), Some(0));
        assert_eq!(cost(1, 500), Some(1));
        assert_eq!(cost(100, i64::MAX), None);
        assert_eq!(peso(i64::MIN), "-₱92233720368547758.08");
    }

    #[test]
    fn minimum_and_cap_apply_to_goods_only() {
        assert!(!permits_goods_subtotal(
            DEFAULT_MINIMUM_CENTAVOS - 1,
            DEFAULT_MINIMUM_CENTAVOS
        ));
        assert!(permits_goods_subtotal(
            DEFAULT_MINIMUM_CENTAVOS,
            DEFAULT_MINIMUM_CENTAVOS
        ));
        assert!(!permits_goods_subtotal(
            MAX_GOODS_CENTAVOS + 1,
            DEFAULT_MINIMUM_CENTAVOS
        ));
    }
}
