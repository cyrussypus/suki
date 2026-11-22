CREATE TABLE user_sessions (
 id CHAR(36) PRIMARY KEY,
 user_id CHAR(36) NOT NULL,
 device_name VARCHAR(120) NOT NULL,
 refresh_token_hash CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL UNIQUE,
 expires_at BIGINT NOT NULL,
 created_at BIGINT NOT NULL,
 last_used_at BIGINT NOT NULL,
 FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
 INDEX(user_id, expires_at), INDEX(expires_at)
) ENGINE=InnoDB;

CREATE TABLE spent_refresh_tokens (
 token_hash CHAR(64) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
 session_id CHAR(36) NOT NULL,
 used_at BIGINT NOT NULL,
 FOREIGN KEY (session_id) REFERENCES user_sessions(id) ON DELETE CASCADE
) ENGINE=InnoDB;
