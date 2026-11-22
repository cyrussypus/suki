ALTER TABLE delivery_locations ADD COLUMN heading DOUBLE NULL, ADD COLUMN speed DOUBLE NULL, ADD COLUMN observed_at BIGINT NULL;
ALTER TABLE vendor_profiles ADD COLUMN reputation_score DECIMAL(3,2) NOT NULL DEFAULT 5.00, ADD COLUMN total_reviews INT UNSIGNED NOT NULL DEFAULT 0, ADD COLUMN reputation_points INT UNSIGNED NOT NULL DEFAULT 0, ADD COLUMN five_star_streak INT UNSIGNED NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN reputation_score DECIMAL(3,2) NOT NULL DEFAULT 5.00, ADD COLUMN total_deliveries INT UNSIGNED NOT NULL DEFAULT 0, ADD COLUMN reputation_points INT UNSIGNED NOT NULL DEFAULT 0, ADD COLUMN five_star_streak INT UNSIGNED NOT NULL DEFAULT 0;
UPDATE users u SET total_deliveries = (SELECT COUNT(*) FROM orders o WHERE o.courier_id=u.id AND o.delivered_at IS NOT NULL);
CREATE TABLE vendor_reviews (
 id CHAR(36) PRIMARY KEY, order_id CHAR(36) NOT NULL UNIQUE, customer_id CHAR(36) NOT NULL, vendor_id CHAR(36) NOT NULL,
 rating TINYINT UNSIGNED NOT NULL CHECK (rating BETWEEN 1 AND 5), comment VARCHAR(1000) NULL, tags JSON NOT NULL, created_at BIGINT NOT NULL,
 FOREIGN KEY(order_id) REFERENCES orders(id), FOREIGN KEY(customer_id) REFERENCES users(id), FOREIGN KEY(vendor_id) REFERENCES vendor_profiles(user_id), INDEX(vendor_id,created_at)
);
CREATE TABLE rider_reviews (
 id CHAR(36) PRIMARY KEY, order_id CHAR(36) NOT NULL UNIQUE, customer_id CHAR(36) NOT NULL, rider_id CHAR(36) NOT NULL,
 rating TINYINT UNSIGNED NOT NULL CHECK (rating BETWEEN 1 AND 5), comment VARCHAR(1000) NULL, tags JSON NOT NULL, created_at BIGINT NOT NULL,
 FOREIGN KEY(order_id) REFERENCES orders(id), FOREIGN KEY(customer_id) REFERENCES users(id), FOREIGN KEY(rider_id) REFERENCES users(id), INDEX(rider_id,created_at)
);
CREATE TRIGGER vendor_review_audit AFTER INSERT ON vendor_reviews FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at) VALUES(NEW.order_id,'vendor_reviews.insert',JSON_OBJECT('id',NEW.id,'customer_id',NEW.customer_id,'vendor_id',NEW.vendor_id,'rating',NEW.rating,'comment_hash',SHA2(COALESCE(NEW.comment,''),256),'tags',NEW.tags,'created_at',NEW.created_at),UNIX_TIMESTAMP());
CREATE TRIGGER rider_review_audit AFTER INSERT ON rider_reviews FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at) VALUES(NEW.order_id,'rider_reviews.insert',JSON_OBJECT('id',NEW.id,'customer_id',NEW.customer_id,'rider_id',NEW.rider_id,'rating',NEW.rating,'comment_hash',SHA2(COALESCE(NEW.comment,''),256),'tags',NEW.tags,'created_at',NEW.created_at),UNIX_TIMESTAMP());
CREATE TRIGGER vendor_review_no_update BEFORE UPDATE ON vendor_reviews FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='Reviews cannot be edited';
CREATE TRIGGER vendor_review_no_delete BEFORE DELETE ON vendor_reviews FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='Reviews cannot be deleted';
CREATE TRIGGER rider_review_no_update BEFORE UPDATE ON rider_reviews FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='Reviews cannot be edited';
CREATE TRIGGER rider_review_no_delete BEFORE DELETE ON rider_reviews FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='Reviews cannot be deleted';
