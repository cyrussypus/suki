Suki Rust Marketplace Application — Complete Setup & Operations Manual

This manual provides an end-to-end technical reference for setting up, configuring, running, and managing the Suki Rust Marketplace Application. Suki is a multi-role market shopping platform tailored for Davao City operations, engineered as a Rust 2024 Cargo workspace. This document details the architectural boundaries, database security initialization, environment configuration, frontend build pipelines, database management techniques, and vendor account provisioning.

1. System Architecture and Crate Responsibilities

The Suki application enforces strict boundaries between client presentation, server REST API handling, and domain logic. The repository is organized as a unified Cargo workspace containing three specialized crates, maintaining memory safety and exact monetary calculations across all platform operations.

Crate Name	Binary / Artifact	Target Platform	Primary Responsibilities
suki-mobile	Client App (crates/suki-mobile)	Android, iOS, Web WASM	Dioxus user interface screens, account handling, in-memory shopping carts, GCash payment reference inputs, order tracking, and private stall messaging.
suki-backend	suki-api (crates/suki-backend)	Linux / macOS Daemon	Axum HTTP REST web API server, SQLx schema migration runner against MariaDB 11.8 LTS, role-based authorization, and ACID order fulfillment.
suki-shared	Shared Library (crates/suki-shared)	Internal Dependency	Shared data models, system event definitions, and centavo integer currency arithmetic.

Crate Boundary Principles

* Client Presentation (suki-mobile): Powered by the Dioxus cross-platform framework. Client state—including active mobile user sessions, product favorites, and shopping cart baskets—is maintained in-memory during execution to minimize round-trip database requests.
* Server Execution (suki-backend): Compiles to the suki-api daemon binary using the Axum web framework. It acts as the single source of truth for marketplace state and database execution.
* Domain Precision (suki-shared): Centralizes domain models and financial calculations. All monetary amounts are computed using exact integer arithmetic (centavos), eliminating floating-point rounding errors during multi-item checkouts.

2. Prerequisites and System Toolchain

Before initiating workspace compilation or database initialization, verify that your local development workstation fulfills the following pinned toolchain standards:

* Programming Language: Rust 2024 Edition.
* Compiler Toolchain: Rust 1.99.1 explicitly pinned in rust-toolchain.toml.
* Database Engine: MariaDB 11.8 LTS.
* Frontend Web Target: wasm32-unknown-unknown target for WebAssembly compilation.
* Third-Party Services: Google Routes API key for calculating delivery distance and logistics pathways.

To install the WebAssembly target required for Dioxus web preview compilation, execute:

rustup target add wasm32-unknown-unknown


3. Database Infrastructure Setup (MariaDB 11.8 LTS)

Suki utilizes MariaDB 11.8 LTS as its persistent storage layer. To maintain database security and avoid running application processes as the system administrator, a dedicated database instance and low-privilege user must be created.

Step 1: Start the MariaDB Daemon

Ensure the local MariaDB background service is active:

* macOS (Homebrew):

brew services start mariadb


* Linux (systemd):

sudo systemctl start mariadb


Step 2: Initialize Database and User Privileges

Connect to MariaDB using system root credentials via socket authentication:

sudo mariadb -u root


Inside the interactive MariaDB [(none)]> prompt, run the following SQL statements sequentially:

-- 1. Create the suki production database with UTF-8 encoding
CREATE DATABASE IF NOT EXISTS suki CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;

-- 2. Create a dedicated application user
CREATE USER IF NOT EXISTS 'suki_user'@'localhost' IDENTIFIED BY 'suki_password';

-- 3. Grant full privileges on the suki database to suki_user
GRANT ALL PRIVILEGES ON suki.* TO 'suki_user'@'localhost';

-- 4. Apply privilege changes and exit
FLUSH PRIVILEGES;
EXIT;


4. Environment Variables and Security Configuration

The backend API server requires two mandatory environment variables at runtime: SUKI_INTEGRITY_KEY for payload authentication and DATABASE_URL for SQLx connection pooling.

To configure your development environment, create a .env file in the repository root directory:

cat << 'EOF' > .env
SUKI_INTEGRITY_KEY="dc9dc1273b3946dd5271825f0dc134d84803a5d7e4def44980715e11a7ef9912"
DATABASE_URL="mysql://suki_user:suki_password@localhost:3306/suki"
EOF


Environment Variable Glossary

* SUKI_INTEGRITY_KEY: Cryptographic hex string required by suki-api to authenticate incoming API requests.
* DATABASE_URL: MySQL connection string formatting database username, password, host, port, and target database name (mysql://<user>:<password>@localhost:3306/suki).
* SUKI_PREVIEW_DIR: Local path pointing to compiled Dioxus WebAssembly static assets ($PWD/target/dx/suki-mobile/debug/web/public).

5. Building and Launching the Application

Setting up the local environment requires building the frontend WebAssembly interface followed by executing the Axum server binary. On startup, the backend automatically runs pending SQLx database migrations against the target MariaDB instance.

Step 1: Compile Dioxus WebAssembly Frontend

From the root workspace directory, build the suki-mobile web assets:

cd crates/suki-mobile && dx build --platform web --features web --locked && cd ../..


Step 2: Launch the Backend API Server (suki-api)

Run the backend service with environment variables and the web preview directory declared:

SUKI_INTEGRITY_KEY="dc9dc1273b3946dd5271825f0dc134d84803a5d7e4def44980715e11a7ef9912" \
DATABASE_URL="mysql://suki_user:suki_password@localhost:3306/suki" \
SUKI_PREVIEW_DIR="$PWD/target/dx/suki-mobile/debug/web/public" \
cargo run --locked -p suki-backend


Once the terminal outputs Running target/debug/suki-api, navigate to http://localhost:8080 in your browser to access the application.

6. Database Inspection and Management (DBeaver & CLI)

Developers can monitor tables, check order records, and inspect schema structures using either the graphical DBeaver client or command-line SQL utilities.

Graphical Management via DBeaver

To launch DBeaver directly from your macOS terminal:

open -a DBeaver


Connection Settings Configuration

1. Click New Database Connection (the plug icon with +) and select MariaDB.
2. Configure the connection parameters:
  * Host: localhost (or 127.0.0.1)
  * Port: 3306
  * Database: suki
  * Username: suki_user
  * Password: suki_password
3. Click Test Connection, install the MariaDB JDBC driver if prompted, and click Finish.
4. In the left Database Navigator pane, navigate to MariaDB → Databases → suki → Tables to inspect populated tables including users, stalls, products, orders, and gcash_transactions.

Direct Command-Line Management (MariaDB CLI)

You can inspect database state directly from your terminal session:

* Interactive MySQL Session:

mariadb -u suki_user -psuki_password suki


* One-Line Table Listing:

mariadb -u suki_user -psuki_password suki -e "SHOW TABLES;"


* Inspect User Records:

mariadb -u suki_user -psuki_password suki -e "SELECT id, name, email, role, created_at FROM users;"


7. User Role Administration and Vendor Provisioning

Suki establishes four isolated platform roles: Customer, Vendor, Rider, and Admin. Each role unlocks specialized user interfaces and operational permissions across the application ecosystem.

Role Overview & Operational Dashboards

* Customer: Browses stall catalogs, curates baskets, places GCash or Cash on Delivery orders, and participates in order chat.
* Vendor: Manages market stall profiles, uploads product photos, defines item prices and weight specifications, and manages real-time inventory availability.
* Rider: Receives order assignment notifications, reviews pickup logistics, and communicates with customers and vendors.
* Admin: Conducts platform oversight, manages user access roles, and reviews system audit logs.

Granting Vendor Privileges to Registered Users

When users register through the mobile interface, their default account role is set to customer. Platform administrators can elevate a user to vendor status using SQL queries.

1. List All Registered Vendors

To check which accounts currently possess vendor privileges:

mariadb -u suki_user -psuki_password suki -e "SELECT id, name, email, role FROM users WHERE role = 'vendor';"


2. Elevate Account to Vendor Role

To promote a user (e.g., rjaysolamo@gmail.com) to a vendor account:

mariadb -u suki_user -psuki_password suki -e "UPDATE users SET role = 'vendor' WHERE email = 'rjaysolamo@gmail.com';"


3. Confirm Vendor Stall Creation

Once elevated, the vendor account gains access to the Vendor Dashboard at http://localhost:8080, allowing the user to create a market stall entry in the stalls table and populate their product inventory.

8. Quality Assurance and Testing Standards

Prior to pushing code changes, developers must run the repository's verification toolchain to ensure compiler compliance, zero clippy warnings, and database integrity.

Workspace Verification Suite

# Verify compilation across all targets
cargo check --workspace --locked

# Enforce strict lints without warnings
cargo clippy --workspace --all-targets --locked -- -D warnings

# Execute standard unit tests
cargo test --locked


Database Integration Testing

Full integration tests requiring active MariaDB connection pooling are executed against a dedicated disposable test database ending in _test:

# Run integration tests against a temporary test database
TEST_DATABASE_URL="mysql://suki_user:suki_password@localhost:3306/suki_test" cargo test --locked -- --include-ignored


(Note: The test framework enforces that TEST_DATABASE_URL targets a database ending with _test to prevent accidental modification of your development data).
