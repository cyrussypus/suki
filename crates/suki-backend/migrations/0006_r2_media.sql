ALTER TABLE users ADD COLUMN avatar_url VARCHAR(512) DEFAULT NULL;
ALTER TABLE products ADD COLUMN image_url VARCHAR(512) DEFAULT NULL;
CREATE TABLE media_objects (
 id CHAR(36) PRIMARY KEY,
 owner_id CHAR(36) NOT NULL,
 product_id BIGINT NULL,
 object_key VARCHAR(200) NOT NULL UNIQUE,
 public_url VARCHAR(512) NOT NULL,
 created_at BIGINT NOT NULL,
 FOREIGN KEY (owner_id) REFERENCES users(id),
 FOREIGN KEY (product_id) REFERENCES products(id)
);
