CREATE TABLE order_snapshots (
 order_id CHAR(36) PRIMARY KEY,
 payload MEDIUMBLOB NOT NULL,
 snapshot_hash CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
 signature CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
 FOREIGN KEY (order_id) REFERENCES orders(id)
) ENGINE=InnoDB;

CREATE TRIGGER checkout_snapshot_no_update BEFORE UPDATE ON order_snapshots FOR EACH ROW
 SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Checkout snapshots are immutable';

CREATE TRIGGER checkout_snapshot_no_delete BEFORE DELETE ON order_snapshots FOR EACH ROW
 SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'Checkout snapshots are immutable';

CREATE TRIGGER checkout_snapshot_audit AFTER INSERT ON order_snapshots FOR EACH ROW
 INSERT INTO order_integrity (order_id,event,payload,created_at)
 VALUES (NEW.order_id, 'order_snapshots.insert',
         JSON_OBJECT('snapshot_hash',NEW.snapshot_hash,'signature',NEW.signature), UNIX_TIMESTAMP());
