-- Expand before moving data with the application key. Leave older migrations intact.
ALTER TABLE users MODIFY status ENUM('pending','active','suspended','deactivated','pending_approval','rejected') NOT NULL DEFAULT 'active';
UPDATE users SET status='pending_approval' WHERE status='pending' AND role IN ('vendor','courier');

CREATE TABLE onboarding_decisions (
 user_id CHAR(36) PRIMARY KEY, actor_id CHAR(36) NOT NULL,
 decision ENUM('active','rejected') NOT NULL, decided_at BIGINT NOT NULL,
 FOREIGN KEY(user_id) REFERENCES users(id), FOREIGN KEY(actor_id) REFERENCES users(id)
) ENGINE=InnoDB;

CREATE TABLE data_encryption_check (id INT PRIMARY KEY, ciphertext VARBINARY(128) NOT NULL) ENGINE=InnoDB;

ALTER TABLE orders ADD delivery_address_encrypted BLOB NULL, ADD total_amount_centavos_encrypted VARBINARY(64) NULL,
 MODIFY address VARCHAR(500) NULL, MODIFY lat DOUBLE NULL, MODIFY lng DOUBLE NULL, MODIFY estimated_total_centavos BIGINT NULL,
 ADD CONSTRAINT orders_storage CHECK (
  (delivery_address_encrypted IS NULL AND total_amount_centavos_encrypted IS NULL AND address IS NOT NULL AND lat IS NOT NULL AND lng IS NOT NULL AND estimated_total_centavos IS NOT NULL)
  OR (delivery_address_encrypted IS NOT NULL AND total_amount_centavos_encrypted IS NOT NULL AND address IS NULL AND lat IS NULL AND lng IS NULL AND estimated_total_centavos IS NULL)
 );
ALTER TABLE delivery_quotes ADD address_encrypted BLOB NULL,
 MODIFY address VARCHAR(500) NULL, MODIFY lat DOUBLE NULL, MODIFY lng DOUBLE NULL,
 ADD CONSTRAINT quote_storage CHECK ((address_encrypted IS NULL AND address IS NOT NULL AND lat IS NOT NULL AND lng IS NOT NULL)
  OR (address_encrypted IS NOT NULL AND address IS NULL AND lat IS NULL AND lng IS NULL));
ALTER TABLE customer_addresses ADD address_encrypted BLOB NULL,
 MODIFY address VARCHAR(500) NULL, MODIFY lat DOUBLE NULL, MODIFY lng DOUBLE NULL, MODIFY label VARCHAR(80) NULL,
 ADD CONSTRAINT saved_address_storage CHECK ((address_encrypted IS NULL AND address IS NOT NULL AND lat IS NOT NULL AND lng IS NOT NULL AND label IS NOT NULL)
  OR (address_encrypted IS NOT NULL AND address IS NULL AND lat IS NULL AND lng IS NULL AND label IS NULL));
ALTER TABLE order_snapshots ADD payload_encrypted MEDIUMBLOB NULL, MODIFY payload MEDIUMBLOB NULL,
 ADD CONSTRAINT snapshot_storage CHECK ((payload IS NULL) <> (payload_encrypted IS NULL));
ALTER TABLE order_integrity ADD payload_encrypted MEDIUMBLOB NULL;

DROP TRIGGER checkout_snapshot_no_update;
CREATE TRIGGER checkout_snapshot_no_update BEFORE UPDATE ON order_snapshots FOR EACH ROW
BEGIN
 IF NOT (OLD.payload_encrypted IS NULL AND NEW.payload_encrypted IS NOT NULL AND NEW.payload IS NULL
     AND BINARY OLD.order_id = BINARY NEW.order_id AND OLD.snapshot_hash = NEW.snapshot_hash AND OLD.signature = NEW.signature) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Checkout snapshots are immutable';
 END IF;
END;

DROP TRIGGER integrity_write_once;
CREATE TRIGGER integrity_write_once BEFORE UPDATE ON order_integrity FOR EACH ROW
BEGIN
 IF NOT (OLD.id = NEW.id AND BINARY OLD.order_id = BINARY NEW.order_id AND
    BINARY OLD.event = BINARY NEW.event AND OLD.created_at = NEW.created_at AND (
     (OLD.tag IS NULL AND NEW.tag IS NOT NULL AND BINARY OLD.payload = BINARY NEW.payload
      AND BINARY OLD.payload_encrypted <=> BINARY NEW.payload_encrypted)
     OR (OLD.payload_encrypted IS NULL AND NEW.payload_encrypted IS NOT NULL AND NEW.payload = '{}'
         AND (OLD.actor_id <=> NEW.actor_id) AND (OLD.tag <=> NEW.tag)
         AND OLD.event IN ('orders.insert','orders.update') AND JSON_CONTAINS_PATH(OLD.payload,'one','$.estimated_total_centavos'))
 )) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Order integrity entries are append-only';
 END IF;
END;

DROP TRIGGER integrity_orders_insert;
DROP TRIGGER integrity_orders_update;
CREATE TRIGGER integrity_orders_insert AFTER INSERT ON orders FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.id, 'orders.insert', JSON_OBJECT(
   'id',NEW.id, 'customer_id',NEW.customer_id, 'courier_id',NEW.courier_id,
   'status',NEW.status, 'payment_status',NEW.payment_status, 'delivery_address_hash',SHA2(NEW.delivery_address_encrypted,256),
   'distance_meters',NEW.distance_meters,
   'delivery_fee_centavos',NEW.delivery_fee_centavos, 'goods_subtotal_centavos',NEW.goods_subtotal_centavos, 'total_amount_hash',SHA2(NEW.total_amount_centavos_encrypted,256),
   'final_goods_centavos',NEW.final_goods_centavos, 'authorized_goods_centavos',NEW.authorized_goods_centavos, 'idempotency_key_hash',SHA2(CAST(NEW.idempotency_key AS CHAR),256),
   'request_hash',NEW.request_hash, 'created_at',NEW.created_at, 'updated_at',NEW.updated_at,
   'delivered_at',NEW.delivered_at, 'payment_method',NEW.payment_method, 'vendor_id',NEW.vendor_id,
   'gcash_number_hash',SHA2(CAST(NEW.gcash_number AS CHAR),256), 'weight_grams',NEW.weight_grams
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_orders_update AFTER UPDATE ON orders FOR EACH ROW
order_update: BEGIN
 IF NOT (BINARY OLD.id <=> BINARY NEW.id) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Cannot move an audited record between orders';
 END IF;
 -- During the offline upgrade only, replace legacy sensitive columns without
 -- generating a business event or sealing existing unsigned events.
 IF OLD.delivery_address_encrypted IS NULL AND NEW.delivery_address_encrypted IS NOT NULL AND
    OLD.total_amount_centavos_encrypted IS NULL AND NEW.total_amount_centavos_encrypted IS NOT NULL AND
    (OLD.customer_id <=> NEW.customer_id) AND
    (OLD.courier_id <=> NEW.courier_id) AND
    (OLD.status <=> NEW.status) AND
    (OLD.payment_status <=> NEW.payment_status) AND
    (OLD.distance_meters <=> NEW.distance_meters) AND
    (OLD.delivery_fee_centavos <=> NEW.delivery_fee_centavos) AND
    (OLD.goods_subtotal_centavos <=> NEW.goods_subtotal_centavos) AND
    (OLD.final_goods_centavos <=> NEW.final_goods_centavos) AND
    (OLD.authorized_goods_centavos <=> NEW.authorized_goods_centavos) AND
    (OLD.idempotency_key <=> NEW.idempotency_key) AND
    (OLD.request_hash <=> NEW.request_hash) AND
    (OLD.created_at <=> NEW.created_at) AND
    (OLD.updated_at <=> NEW.updated_at) AND
    (OLD.delivered_at <=> NEW.delivered_at) AND
    (OLD.payment_method <=> NEW.payment_method) AND
    (OLD.vendor_id <=> NEW.vendor_id) AND
    (OLD.gcash_number <=> NEW.gcash_number) AND
    (OLD.weight_grams <=> NEW.weight_grams) THEN
   LEAVE order_update;
 END IF;
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.id, 'orders.update', JSON_OBJECT(
   'id',NEW.id, 'customer_id',NEW.customer_id, 'courier_id',NEW.courier_id,
   'status',NEW.status, 'payment_status',NEW.payment_status, 'delivery_address_hash',SHA2(NEW.delivery_address_encrypted,256),
   'distance_meters',NEW.distance_meters,
   'delivery_fee_centavos',NEW.delivery_fee_centavos, 'goods_subtotal_centavos',NEW.goods_subtotal_centavos, 'total_amount_hash',SHA2(NEW.total_amount_centavos_encrypted,256),
   'final_goods_centavos',NEW.final_goods_centavos, 'authorized_goods_centavos',NEW.authorized_goods_centavos, 'idempotency_key_hash',SHA2(CAST(NEW.idempotency_key AS CHAR),256),
   'request_hash',NEW.request_hash, 'created_at',NEW.created_at, 'updated_at',NEW.updated_at,
   'delivered_at',NEW.delivered_at, 'payment_method',NEW.payment_method, 'vendor_id',NEW.vendor_id,
   'gcash_number_hash',SHA2(CAST(NEW.gcash_number AS CHAR),256), 'weight_grams',NEW.weight_grams
 ), UNIX_TIMESTAMP());
END;


CREATE TRIGGER protected_order_insert BEFORE INSERT ON orders FOR EACH ROW
BEGIN
 IF NEW.delivery_address_encrypted IS NULL THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Order fields must be encrypted';
 END IF;
END;
CREATE TRIGGER protected_order_update BEFORE UPDATE ON orders FOR EACH ROW
BEGIN
 IF OLD.delivery_address_encrypted IS NOT NULL AND NEW.delivery_address_encrypted IS NULL THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Order fields must stay encrypted';
 END IF;
END;
CREATE TRIGGER protected_quote_insert BEFORE INSERT ON delivery_quotes FOR EACH ROW
BEGIN
 IF NEW.address_encrypted IS NULL THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Quote address must be encrypted';
 END IF;
END;
CREATE TRIGGER protected_quote_update BEFORE UPDATE ON delivery_quotes FOR EACH ROW
BEGIN
 IF OLD.address_encrypted IS NOT NULL AND NEW.address_encrypted IS NULL THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Quote address must stay encrypted';
 END IF;
END;
CREATE TRIGGER protected_address_insert BEFORE INSERT ON customer_addresses FOR EACH ROW
BEGIN
 IF NEW.address_encrypted IS NULL THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Saved address must be encrypted';
 END IF;
END;
CREATE TRIGGER protected_address_update BEFORE UPDATE ON customer_addresses FOR EACH ROW
BEGIN
 IF OLD.address_encrypted IS NOT NULL AND NEW.address_encrypted IS NULL THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Saved address must stay encrypted';
 END IF;
END;
CREATE TRIGGER protected_snapshot_insert BEFORE INSERT ON order_snapshots FOR EACH ROW
BEGIN
 IF NEW.payload_encrypted IS NULL THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Snapshot must be encrypted';
 END IF;
END;
