-- The database captures row images inside the writing transaction. The API
-- seals the captured entries before commit with a key kept outside MariaDB.
CREATE TABLE order_integrity (
 id BIGINT PRIMARY KEY AUTO_INCREMENT,
 order_id CHAR(36) NOT NULL,
 event VARCHAR(40) NOT NULL,
 payload JSON NOT NULL,
 created_at BIGINT NOT NULL,
 actor_id CHAR(36),
 tag CHAR(64) CHARACTER SET ascii COLLATE ascii_bin,
 FOREIGN KEY (order_id) REFERENCES orders(id),
 INDEX by_order (order_id,id),
 INDEX pending_by_order (order_id,tag,id),
 CHECK ((actor_id IS NULL) = (tag IS NULL))
) ENGINE=InnoDB;

CREATE TRIGGER integrity_write_once BEFORE UPDATE ON order_integrity FOR EACH ROW
BEGIN
 IF OLD.tag IS NOT NULL OR NEW.tag IS NULL OR NOT (
    OLD.id = NEW.id AND BINARY OLD.order_id = BINARY NEW.order_id AND
    BINARY OLD.event = BINARY NEW.event AND BINARY OLD.payload = BINARY NEW.payload AND
    OLD.created_at = NEW.created_at
 ) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Order integrity entries are append-only';
 END IF;
END;
CREATE TRIGGER integrity_no_delete BEFORE DELETE ON order_integrity FOR EACH ROW
 SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Order integrity entries are append-only';

ALTER TABLE payments ADD reference_hash CHAR(64) CHARACTER SET ascii COLLATE ascii_bin
 AS (CASE WHEN LEFT(reference,4) <> 'cod:' THEN SHA2(UPPER(TRIM(reference)),256) END)
 STORED UNIQUE;

ALTER TABLE chat_messages
 ADD nonce CHAR(36) CHARACTER SET ascii COLLATE ascii_bin UNIQUE,
 ADD hash CHAR(64) CHARACTER SET ascii COLLATE ascii_bin,
 ADD signature CHAR(64) CHARACTER SET ascii COLLATE ascii_bin;
CREATE TRIGGER chat_requires_integrity BEFORE INSERT ON chat_messages FOR EACH ROW
BEGIN
 IF NEW.nonce IS NULL OR NEW.hash IS NULL OR NEW.signature IS NULL THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'New chat messages need integrity metadata';
 END IF;
END;
CREATE TRIGGER chat_no_edits BEFORE UPDATE ON chat_messages FOR EACH ROW
 SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Chat messages cannot be edited';

CREATE TRIGGER integrity_orders_insert AFTER INSERT ON orders FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.id, 'orders.insert', JSON_OBJECT(
   'id',NEW.id, 'customer_id',NEW.customer_id, 'courier_id',NEW.courier_id,
   'status',NEW.status, 'payment_status',NEW.payment_status, 'address_hash',SHA2(CAST(NEW.address AS CHAR),256),
   'lat_hash',SHA2(CAST(NEW.lat AS CHAR),256), 'lng_hash',SHA2(CAST(NEW.lng AS CHAR),256), 'distance_meters',NEW.distance_meters,
   'delivery_fee_centavos',NEW.delivery_fee_centavos, 'goods_subtotal_centavos',NEW.goods_subtotal_centavos, 'estimated_total_centavos',NEW.estimated_total_centavos,
   'final_goods_centavos',NEW.final_goods_centavos, 'authorized_goods_centavos',NEW.authorized_goods_centavos, 'idempotency_key_hash',SHA2(CAST(NEW.idempotency_key AS CHAR),256),
   'request_hash',NEW.request_hash, 'created_at',NEW.created_at, 'updated_at',NEW.updated_at,
   'delivered_at',NEW.delivered_at, 'payment_method',NEW.payment_method, 'vendor_id',NEW.vendor_id,
   'gcash_number_hash',SHA2(CAST(NEW.gcash_number AS CHAR),256), 'weight_grams',NEW.weight_grams
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_orders_update AFTER UPDATE ON orders FOR EACH ROW
BEGIN
 IF NOT (BINARY OLD.id <=> BINARY NEW.id) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Cannot move an audited record between orders';
 END IF;
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.id, 'orders.update', JSON_OBJECT(
   'id',NEW.id, 'customer_id',NEW.customer_id, 'courier_id',NEW.courier_id,
   'status',NEW.status, 'payment_status',NEW.payment_status, 'address_hash',SHA2(CAST(NEW.address AS CHAR),256),
   'lat_hash',SHA2(CAST(NEW.lat AS CHAR),256), 'lng_hash',SHA2(CAST(NEW.lng AS CHAR),256), 'distance_meters',NEW.distance_meters,
   'delivery_fee_centavos',NEW.delivery_fee_centavos, 'goods_subtotal_centavos',NEW.goods_subtotal_centavos, 'estimated_total_centavos',NEW.estimated_total_centavos,
   'final_goods_centavos',NEW.final_goods_centavos, 'authorized_goods_centavos',NEW.authorized_goods_centavos, 'idempotency_key_hash',SHA2(CAST(NEW.idempotency_key AS CHAR),256),
   'request_hash',NEW.request_hash, 'created_at',NEW.created_at, 'updated_at',NEW.updated_at,
   'delivered_at',NEW.delivered_at, 'payment_method',NEW.payment_method, 'vendor_id',NEW.vendor_id,
   'gcash_number_hash',SHA2(CAST(NEW.gcash_number AS CHAR),256), 'weight_grams',NEW.weight_grams
 ), UNIX_TIMESTAMP());
END;

CREATE TRIGGER integrity_orders_no_delete BEFORE DELETE ON orders FOR EACH ROW
 SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Orders with integrity tracking cannot be deleted';

CREATE TRIGGER integrity_order_items_insert AFTER INSERT ON order_items FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'order_items.insert', JSON_OBJECT(
   'id',NEW.id, 'product_id',NEW.product_id, 'vendor_id',NEW.vendor_id,
   'name',NEW.name, 'unit',NEW.unit, 'unit_price_centavos',NEW.unit_price_centavos,
   'quantity_milli',NEW.quantity_milli, 'estimated_cost_centavos',NEW.estimated_cost_centavos, 'actual_quantity_milli',NEW.actual_quantity_milli,
   'actual_unit_price_centavos',NEW.actual_unit_price_centavos, 'actual_cost_centavos',NEW.actual_cost_centavos, 'unavailable',NEW.unavailable,
   'available_quantity_milli',NEW.available_quantity_milli
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_order_items_update AFTER UPDATE ON order_items FOR EACH ROW
BEGIN
 IF NOT (BINARY OLD.order_id <=> BINARY NEW.order_id) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Cannot move an audited record between orders';
 END IF;
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'order_items.update', JSON_OBJECT(
   'id',NEW.id, 'product_id',NEW.product_id, 'vendor_id',NEW.vendor_id,
   'name',NEW.name, 'unit',NEW.unit, 'unit_price_centavos',NEW.unit_price_centavos,
   'quantity_milli',NEW.quantity_milli, 'estimated_cost_centavos',NEW.estimated_cost_centavos, 'actual_quantity_milli',NEW.actual_quantity_milli,
   'actual_unit_price_centavos',NEW.actual_unit_price_centavos, 'actual_cost_centavos',NEW.actual_cost_centavos, 'unavailable',NEW.unavailable,
   'available_quantity_milli',NEW.available_quantity_milli
 ), UNIX_TIMESTAMP());
END;

CREATE TRIGGER integrity_order_items_delete AFTER DELETE ON order_items FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (OLD.order_id, 'order_items.delete', JSON_OBJECT(
   'id',OLD.id, 'product_id',OLD.product_id, 'vendor_id',OLD.vendor_id,
   'name',OLD.name, 'unit',OLD.unit, 'unit_price_centavos',OLD.unit_price_centavos,
   'quantity_milli',OLD.quantity_milli, 'estimated_cost_centavos',OLD.estimated_cost_centavos, 'actual_quantity_milli',OLD.actual_quantity_milli,
   'actual_unit_price_centavos',OLD.actual_unit_price_centavos, 'actual_cost_centavos',OLD.actual_cost_centavos, 'unavailable',OLD.unavailable,
   'available_quantity_milli',OLD.available_quantity_milli
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_payments_insert AFTER INSERT ON payments FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'payments.insert', JSON_OBJECT(
   'id',NEW.id, 'reference_hash',NEW.reference_hash, 'claimed_amount_centavos',NEW.claimed_amount_centavos,
   'received_amount_centavos',NEW.received_amount_centavos, 'status',NEW.status, 'verified_by',NEW.verified_by,
   'created_at',NEW.created_at, 'verified_at',NEW.verified_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_payments_update AFTER UPDATE ON payments FOR EACH ROW
BEGIN
 IF NOT (BINARY OLD.order_id <=> BINARY NEW.order_id) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Cannot move an audited record between orders';
 END IF;
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'payments.update', JSON_OBJECT(
   'id',NEW.id, 'reference_hash',NEW.reference_hash, 'claimed_amount_centavos',NEW.claimed_amount_centavos,
   'received_amount_centavos',NEW.received_amount_centavos, 'status',NEW.status, 'verified_by',NEW.verified_by,
   'created_at',NEW.created_at, 'verified_at',NEW.verified_at
 ), UNIX_TIMESTAMP());
END;

CREATE TRIGGER integrity_payments_delete AFTER DELETE ON payments FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (OLD.order_id, 'payments.delete', JSON_OBJECT(
   'id',OLD.id, 'reference_hash',OLD.reference_hash, 'claimed_amount_centavos',OLD.claimed_amount_centavos,
   'received_amount_centavos',OLD.received_amount_centavos, 'status',OLD.status, 'verified_by',OLD.verified_by,
   'created_at',OLD.created_at, 'verified_at',OLD.verified_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_refunds_insert AFTER INSERT ON refunds FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'refunds.insert', JSON_OBJECT(
   'id',NEW.id, 'amount_centavos',NEW.amount_centavos, 'status',NEW.status,
   'reference_hash',SHA2(CAST(NEW.reference AS CHAR),256), 'processed_by',NEW.processed_by, 'created_at',NEW.created_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_refunds_update AFTER UPDATE ON refunds FOR EACH ROW
BEGIN
 IF NOT (BINARY OLD.order_id <=> BINARY NEW.order_id) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Cannot move an audited record between orders';
 END IF;
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'refunds.update', JSON_OBJECT(
   'id',NEW.id, 'amount_centavos',NEW.amount_centavos, 'status',NEW.status,
   'reference_hash',SHA2(CAST(NEW.reference AS CHAR),256), 'processed_by',NEW.processed_by, 'created_at',NEW.created_at
 ), UNIX_TIMESTAMP());
END;

CREATE TRIGGER integrity_refunds_delete AFTER DELETE ON refunds FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (OLD.order_id, 'refunds.delete', JSON_OBJECT(
   'id',OLD.id, 'amount_centavos',OLD.amount_centavos, 'status',OLD.status,
   'reference_hash',SHA2(CAST(OLD.reference AS CHAR),256), 'processed_by',OLD.processed_by, 'created_at',OLD.created_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_vendor_settlements_insert AFTER INSERT ON vendor_settlements FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'vendor_settlements.insert', JSON_OBJECT(
   'amount_centavos',NEW.amount_centavos, 'reference_hash',SHA2(CAST(NEW.reference AS CHAR),256), 'recorded_by',NEW.recorded_by,
   'recorded_at',NEW.recorded_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_vendor_settlements_update AFTER UPDATE ON vendor_settlements FOR EACH ROW
BEGIN
 IF NOT (BINARY OLD.order_id <=> BINARY NEW.order_id) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Cannot move an audited record between orders';
 END IF;
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'vendor_settlements.update', JSON_OBJECT(
   'amount_centavos',NEW.amount_centavos, 'reference_hash',SHA2(CAST(NEW.reference AS CHAR),256), 'recorded_by',NEW.recorded_by,
   'recorded_at',NEW.recorded_at
 ), UNIX_TIMESTAMP());
END;

CREATE TRIGGER integrity_vendor_settlements_delete AFTER DELETE ON vendor_settlements FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (OLD.order_id, 'vendor_settlements.delete', JSON_OBJECT(
   'amount_centavos',OLD.amount_centavos, 'reference_hash',SHA2(CAST(OLD.reference AS CHAR),256), 'recorded_by',OLD.recorded_by,
   'recorded_at',OLD.recorded_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_adjustments_insert AFTER INSERT ON adjustments FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'adjustments.insert', JSON_OBJECT(
   'id',NEW.id, 'proposed_goods_centavos',NEW.proposed_goods_centavos, 'reason_hash',SHA2(CAST(NEW.reason AS CHAR),256),
   'status',NEW.status, 'customer_decided_at',NEW.customer_decided_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_adjustments_update AFTER UPDATE ON adjustments FOR EACH ROW
BEGIN
 IF NOT (BINARY OLD.order_id <=> BINARY NEW.order_id) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Cannot move an audited record between orders';
 END IF;
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'adjustments.update', JSON_OBJECT(
   'id',NEW.id, 'proposed_goods_centavos',NEW.proposed_goods_centavos, 'reason_hash',SHA2(CAST(NEW.reason AS CHAR),256),
   'status',NEW.status, 'customer_decided_at',NEW.customer_decided_at
 ), UNIX_TIMESTAMP());
END;

CREATE TRIGGER integrity_adjustments_delete AFTER DELETE ON adjustments FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (OLD.order_id, 'adjustments.delete', JSON_OBJECT(
   'id',OLD.id, 'proposed_goods_centavos',OLD.proposed_goods_centavos, 'reason_hash',SHA2(CAST(OLD.reason AS CHAR),256),
   'status',OLD.status, 'customer_decided_at',OLD.customer_decided_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_substitutions_insert AFTER INSERT ON substitutions FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'substitutions.insert', JSON_OBJECT(
   'id',NEW.id, 'item_id',NEW.item_id, 'name',NEW.name,
   'unit',NEW.unit, 'quantity_milli',NEW.quantity_milli, 'unit_price_centavos',NEW.unit_price_centavos,
   'status',NEW.status, 'decided_at',NEW.decided_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_substitutions_update AFTER UPDATE ON substitutions FOR EACH ROW
BEGIN
 IF NOT (BINARY OLD.order_id <=> BINARY NEW.order_id) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Cannot move an audited record between orders';
 END IF;
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'substitutions.update', JSON_OBJECT(
   'id',NEW.id, 'item_id',NEW.item_id, 'name',NEW.name,
   'unit',NEW.unit, 'quantity_milli',NEW.quantity_milli, 'unit_price_centavos',NEW.unit_price_centavos,
   'status',NEW.status, 'decided_at',NEW.decided_at
 ), UNIX_TIMESTAMP());
END;

CREATE TRIGGER integrity_substitutions_delete AFTER DELETE ON substitutions FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (OLD.order_id, 'substitutions.delete', JSON_OBJECT(
   'id',OLD.id, 'item_id',OLD.item_id, 'name',OLD.name,
   'unit',OLD.unit, 'quantity_milli',OLD.quantity_milli, 'unit_price_centavos',OLD.unit_price_centavos,
   'status',OLD.status, 'decided_at',OLD.decided_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_expenses_insert AFTER INSERT ON expenses FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'expenses.insert', JSON_OBJECT(
   'id',NEW.id, 'amount_centavos',NEW.amount_centavos, 'description_hash',SHA2(CAST(NEW.description AS CHAR),256),
   'actor_id',NEW.actor_id, 'created_at',NEW.created_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_expenses_update AFTER UPDATE ON expenses FOR EACH ROW
BEGIN
 IF NOT (BINARY OLD.order_id <=> BINARY NEW.order_id) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Cannot move an audited record between orders';
 END IF;
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'expenses.update', JSON_OBJECT(
   'id',NEW.id, 'amount_centavos',NEW.amount_centavos, 'description_hash',SHA2(CAST(NEW.description AS CHAR),256),
   'actor_id',NEW.actor_id, 'created_at',NEW.created_at
 ), UNIX_TIMESTAMP());
END;

CREATE TRIGGER integrity_expenses_delete AFTER DELETE ON expenses FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (OLD.order_id, 'expenses.delete', JSON_OBJECT(
   'id',OLD.id, 'amount_centavos',OLD.amount_centavos, 'description_hash',SHA2(CAST(OLD.description AS CHAR),256),
   'actor_id',OLD.actor_id, 'created_at',OLD.created_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_order_status_history_insert AFTER INSERT ON order_status_history FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'order_status_history.insert', JSON_OBJECT(
   'id',NEW.id, 'status',NEW.status, 'actor_id',NEW.actor_id,
   'created_at',NEW.created_at
 ), UNIX_TIMESTAMP());

CREATE TRIGGER integrity_order_status_history_update AFTER UPDATE ON order_status_history FOR EACH ROW
BEGIN
 IF NOT (BINARY OLD.order_id <=> BINARY NEW.order_id) THEN
   SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Cannot move an audited record between orders';
 END IF;
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'order_status_history.update', JSON_OBJECT(
   'id',NEW.id, 'status',NEW.status, 'actor_id',NEW.actor_id,
   'created_at',NEW.created_at
 ), UNIX_TIMESTAMP());
END;

CREATE TRIGGER integrity_order_status_history_delete AFTER DELETE ON order_status_history FOR EACH ROW
 INSERT INTO order_integrity(order_id,event,payload,created_at)
 VALUES (OLD.order_id, 'order_status_history.delete', JSON_OBJECT(
   'id',OLD.id, 'status',OLD.status, 'actor_id',OLD.actor_id,
   'created_at',OLD.created_at
 ), UNIX_TIMESTAMP());
