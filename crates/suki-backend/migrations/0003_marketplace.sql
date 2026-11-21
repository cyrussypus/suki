ALTER TABLE products ADD weight_grams BIGINT NOT NULL DEFAULT 1000 CHECK(weight_grams BETWEEN 1 AND 50000);
ALTER TABLE vendor_profiles ADD gcash_number VARCHAR(11) NULL;
ALTER TABLE delivery_quotes ADD weight_grams BIGINT NULL CHECK(weight_grams BETWEEN 1 AND 100000);
ALTER TABLE orders
 ADD payment_method VARCHAR(20) NOT NULL DEFAULT 'gcash' CHECK(payment_method IN ('gcash','cash_on_delivery')),
 ADD vendor_id CHAR(36) NULL,
 ADD gcash_number VARCHAR(11) NULL,
 ADD weight_grams BIGINT NULL,
 ADD CONSTRAINT fk_marketplace_order_vendor FOREIGN KEY(vendor_id) REFERENCES vendor_profiles(user_id);
ALTER TABLE chat_messages ADD sender_role VARCHAR(20) NULL;
CREATE TABLE vendor_settlements (
 order_id CHAR(36) PRIMARY KEY, amount_centavos BIGINT NOT NULL CHECK(amount_centavos >= 0),
 reference VARCHAR(100) NOT NULL, recorded_by CHAR(36) NOT NULL, recorded_at BIGINT NOT NULL,
 FOREIGN KEY(order_id) REFERENCES orders(id), FOREIGN KEY(recorded_by) REFERENCES users(id)
);
