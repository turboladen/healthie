# M1a — `healthie-shared` Domain Library Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A tested, HTTP-agnostic Rust domain library holding healthie's M1 schema (Profile, Concern, Goal, Protocol, Observation, Checkin, Plan) and services, culminating in the checkin-briefing assembler.

**Architecture:** Cargo workspace with one lib crate `healthie-shared`, copied from glovebox's proven shape: SeaORM entities + migrations + free-function services taking `&impl ConnectionTrait`, `DomainError` thiserror enum, `NewX`/`UpdateX` input structs, in-memory SQLite `test_db()` behind a `test-support` feature. No axum, no rmcp in this plan — `healthie-mcp` is plan M1b.

**Tech Stack:** Rust edition 2024, SeaORM 1.1 (sqlx-sqlite, runtime-tokio-rustls, macros, with-chrono), sea-orm-migration 1.1, tokio 1, thiserror 2, serde 1, chrono 0.4, tempfile 3 (dev).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-16-healthie-vision-reset-design.md`. Reference conventions: `../glovebox` (copy), `../kammerz` (deploy/ops only, later).
- Business logic lives in services, never in future handlers/tools.
- IDs are `i32` auto-increment PKs. Timestamps are TEXT columns as Rust `String`, format `%Y-%m-%d %H:%M:%S` (UTC); date-only fields are `YYYY-MM-DD` strings.
- Enum-like values are plain `String` columns validated against service-layer whitelists (glovebox style); invalid values → `DomainError::BadRequest` listing allowed values.
- Every service fn returns `DomainResult<T>`; missing rows → `DomainError::NotFound`.
- Crate roots carry `#![allow(clippy::option_option, clippy::struct_field_names, clippy::wildcard_imports)]`.
- Quality gates per task: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D clippy::pedantic` pass before each commit.
- Commit messages end with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Do NOT commit `.beads/issues.jsonl` churn with feature commits (`git restore --staged .beads/issues.jsonl` if staged).

## File structure (locked)

```
Cargo.toml                          # workspace: members = ["healthie-shared"], workspace.dependencies
rustfmt.toml                        # imports_granularity = "Crate", format_strings = true (nightly fmt)
Justfile                            # fmt, fmt-check, ci
healthie-shared/
├── Cargo.toml                      # lib name healthie_shared, test-support feature
└── src/
    ├── lib.rs                      # module decls + crate allows
    ├── error.rs                    # DomainError, DomainResult
    ├── clock.rs                    # now_str(), today_str()
    ├── test_support.rs             # test_db() (feature-gated)
    ├── migration/
    │   ├── mod.rs                  # Migrator
    │   └── m20260716_000001_initial_schema.rs   # all M1 tables
    ├── entities/
    │   ├── mod.rs
    │   ├── profile.rs  concern.rs  concern_tag.rs  goal.rs  protocol.rs
    │   ├── observation.rs  checkin.rs  checkin_response.rs
    │   └── plan.rs  plan_item.rs  plan_item_outcome.rs
    ├── inputs/
    │   ├── mod.rs
    │   └── (one file per aggregate: profile.rs, concern.rs, goal.rs, protocol.rs,
    │        observation.rs, plan.rs)
    └── services/
        ├── mod.rs
        ├── profile.rs  concern.rs  goal.rs  protocol.rs  observation.rs
        ├── checkin.rs  plan.rs
        └── briefing.rs             # the product: assemble()
```

One migration file for the whole M1 schema (greenfield; no back-compat concerns). Services own validation; entities are dumb rows.

---

### Task 1: Workspace scaffold + error module

**Files:**
- Create: `Cargo.toml`, `rustfmt.toml`, `Justfile`, `.gitignore` (append), `healthie-shared/Cargo.toml`, `healthie-shared/src/lib.rs`, `healthie-shared/src/error.rs`, `healthie-shared/src/clock.rs`

**Interfaces:**
- Produces: `DomainError` (variants `NotFound(String)`, `Invalid{field,message}`, `BadRequest(String)`, `Db(sea_orm::DbErr)`, `Internal(String)`), `DomainResult<T>`, `DomainError::invalid(field, message)` helper, `clock::now_str() -> String`, `clock::today_str() -> String`. All later tasks consume these.

- [ ] **Step 1: Root `Cargo.toml`**

```toml
[workspace]
resolver = "3"
members = ["healthie-shared"]

[workspace.package]
edition = "2024"
version = "0.1.0"

[workspace.dependencies]
sea-orm = { version = "1.1", features = ["sqlx-sqlite", "runtime-tokio-rustls", "macros", "with-chrono"] }
sea-orm-migration = "1.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
tokio = { version = "1", features = ["full"] }
thiserror = "2"
anyhow = "1"
tracing = "0.1"

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
```

- [ ] **Step 2: `rustfmt.toml`, `Justfile`, `.gitignore`**

`rustfmt.toml`:
```toml
imports_granularity = "Crate"
format_strings = true
```

`Justfile`:
```just
fmt:
    cargo +nightly fmt --all

fmt-check:
    cargo +nightly fmt --all --check

ci:
    cargo build --workspace --locked
    cargo test --workspace --locked
    cargo clippy --workspace --all-targets -- -D clippy::pedantic
```

Append to `.gitignore`:
```
target/
*.db
*.db-shm
*.db-wal
```

- [ ] **Step 3: `healthie-shared/Cargo.toml`**

```toml
[package]
name = "healthie-shared"
edition.workspace = true
version.workspace = true

[lib]
name = "healthie_shared"

[features]
test-support = []

[dependencies]
sea-orm.workspace = true
sea-orm-migration.workspace = true
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tokio.workspace = true
tempfile = "3"
```

- [ ] **Step 4: `src/lib.rs`, `src/error.rs`, `src/clock.rs`**

`src/lib.rs`:
```rust
#![allow(clippy::option_option, clippy::struct_field_names, clippy::wildcard_imports)]

pub mod clock;
pub mod error;
```

`src/error.rs`:
```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("{0}")]
    NotFound(String),
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("{0}")]
    BadRequest(String),
    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
    #[error("{0}")]
    Internal(String),
}

pub type DomainResult<T> = Result<T, DomainError>;

impl DomainError {
    pub fn invalid(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid { field: field.into(), message: message.into() }
    }
}
```

`src/clock.rs`:
```rust
/// UTC timestamp string, the canonical DB format.
pub fn now_str() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// UTC date string `YYYY-MM-DD`.
pub fn today_str() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}
```

- [ ] **Step 5: Verify and commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D clippy::pedantic`
Expected: builds, 0 tests pass, no clippy errors.

```bash
git add Cargo.toml Cargo.lock rustfmt.toml Justfile .gitignore healthie-shared
git commit -m "feat: healthie workspace scaffold with healthie-shared error/clock core

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Initial schema migration + test_db harness

**Files:**
- Create: `healthie-shared/src/migration/mod.rs`, `healthie-shared/src/migration/m20260716_000001_initial_schema.rs`, `healthie-shared/src/test_support.rs`
- Modify: `healthie-shared/src/lib.rs`

**Interfaces:**
- Produces: `migration::Migrator` (MigratorTrait), `test_support::test_db() -> DatabaseConnection` (in-memory SQLite, single conn, migrated). Tables (all with TEXT `created_at`/`updated_at` defaulting to `(datetime('now'))`): `profile`, `concerns`, `concern_tags`, `goals`, `protocols`, `observations`, `checkins`, `checkin_responses`, `plans`, `plan_items`, `plan_item_outcomes`. Column lists below are the single source of truth for every later task.

- [ ] **Step 1: Write the failing test**

In `healthie-shared/src/test_support.rs`:
```rust
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::migration::Migrator;

/// In-memory SQLite, single pinned connection, fully migrated.
pub async fn test_db() -> DatabaseConnection {
    let mut opt = ConnectOptions::new("sqlite::memory:");
    opt.max_connections(1).min_connections(1).sqlx_logging(false);
    let db = Database::connect(opt).await.expect("connect in-memory sqlite");
    Migrator::up(&db, None).await.expect("run migrations");
    db
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Statement};

    #[tokio::test]
    async fn migrations_create_all_m1_tables() {
        let db = super::test_db().await;
        for table in [
            "profile", "concerns", "concern_tags", "goals", "protocols",
            "observations", "checkins", "checkin_responses", "plans",
            "plan_items", "plan_item_outcomes",
        ] {
            let row = db
                .query_one(Statement::from_string(
                    db.get_database_backend(),
                    format!("SELECT name FROM sqlite_master WHERE type='table' AND name='{table}'"),
                ))
                .await
                .unwrap();
            assert!(row.is_some(), "missing table {table}");
        }
    }
}
```

Wire modules in `src/lib.rs` (final form for this task):
```rust
#![allow(clippy::option_option, clippy::struct_field_names, clippy::wildcard_imports)]

pub mod clock;
pub mod error;
pub mod migration;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p healthie-shared migrations_create_all -- --nocapture`
Expected: FAIL to compile — `migration` module missing. That is the red state.

- [ ] **Step 3: Write the migration**

`healthie-shared/src/migration/mod.rs`:
```rust
use sea_orm_migration::prelude::*;

mod m20260716_000001_initial_schema;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260716_000001_initial_schema::Migration)]
    }
}
```

(Note: `async_trait` is re-exported by `sea_orm_migration::prelude` as `sea_orm_migration::async_trait`; if the plain path fails, use `#[sea_orm_migration::async_trait::async_trait]`.)

`healthie-shared/src/migration/m20260716_000001_initial_schema.rs` — the full M1 schema. Pattern per table shown in full for `profile` and `concerns`; repeat mechanically for the rest using the column lists that follow.

```rust
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

fn timestamps(t: &mut TableCreateStatement) -> &mut TableCreateStatement {
    t.col(
        ColumnDef::new(Alias::new("created_at"))
            .text()
            .not_null()
            .default(Expr::cust("(datetime('now'))")),
    )
    .col(
        ColumnDef::new(Alias::new("updated_at"))
            .text()
            .not_null()
            .default(Expr::cust("(datetime('now'))")),
    )
}

fn pk(t: &mut TableCreateStatement, id: Alias) -> &mut TableCreateStatement {
    t.col(ColumnDef::new(id).integer().not_null().auto_increment().primary_key())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // profile — singleton row (id always 1)
        let mut t = Table::create();
        t.table(Alias::new("profile")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(ColumnDef::new(Alias::new("date_of_birth")).text())
            .col(ColumnDef::new(Alias::new("sex")).text())
            .col(ColumnDef::new(Alias::new("height_cm")).integer())
            .col(ColumnDef::new(Alias::new("notes")).text());
        timestamps(&mut t);
        manager.create_table(t).await?;

        // concerns
        let mut t = Table::create();
        t.table(Alias::new("concerns")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(ColumnDef::new(Alias::new("name")).text().not_null())
            .col(ColumnDef::new(Alias::new("status")).text().not_null().default("active"))
            .col(ColumnDef::new(Alias::new("narrative")).text())
            .col(ColumnDef::new(Alias::new("opened_on")).text().not_null())
            .col(ColumnDef::new(Alias::new("resolved_on")).text());
        timestamps(&mut t);
        manager.create_table(t).await?;

        // ... remaining tables follow the identical pattern with these columns:
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            "plan_item_outcomes", "plan_items", "plans", "checkin_responses",
            "checkins", "observations", "protocols", "goals", "concern_tags",
            "concerns", "profile",
        ] {
            manager
                .drop_table(Table::drop().table(Alias::new(table)).if_exists().to_owned())
                .await?;
        }
        Ok(())
    }
}
```

Remaining tables (implement each in `up()` with the same `pk` + `timestamps` helpers; FK = `ForeignKey::create()` inline via `.foreign_key(ForeignKeyCreateStatement::new() ...)` — copy glovebox's inline style):

- `concern_tags`: `id` pk; `concern_id` integer not_null FK→`concerns.id` ON DELETE CASCADE; `tag` text not_null. Unique index on (`concern_id`,`tag`).
- `goals`: `id` pk; `concern_id` integer nullable FK→`concerns.id` ON DELETE SET NULL; `title` text not_null; `description` text; `metric_kind` text; `comparison` text; `target_value` real; `target_high` real; `target_date` text; `status` text not_null default `"active"`; timestamps.
- `protocols`: `id` pk; `concern_id` integer nullable FK→`concerns.id` SET NULL; `goal_id` integer nullable FK→`goals.id` SET NULL; `name` text not_null; `kind` text not_null; `purpose` text; `schedule` text; `started_on` text not_null; `ended_on` text; `review_by` text; `verdict` text; `verdict_rationale` text; timestamps.
- `observations`: `id` pk; `occurred_at` text not_null; `origin` text not_null; `kind` text not_null default `"note"`; `body` text not_null; `severity` integer; `concern_id` integer nullable FK→`concerns.id` SET NULL; `reviewed` integer not_null default `0`; timestamps.
- `checkins`: `id` pk; `started_at` text not_null; `completed_at` text; `summary` text; timestamps.
- `checkin_responses`: `id` pk; `checkin_id` integer not_null FK→`checkins.id` CASCADE; `question` text not_null; `answer` text not_null; `concern_id` integer nullable FK→`concerns.id` SET NULL; timestamps.
- `plans`: `id` pk; `checkin_id` integer nullable FK→`checkins.id` SET NULL; `starts_on` text not_null; `horizon_days` integer not_null default `7`; `guidance` text; `nutrition` text; timestamps.
- `plan_items`: `id` pk; `plan_id` integer not_null FK→`plans.id` CASCADE; `kind` text not_null; `title` text not_null; `detail` text; `scheduled_for` text; timestamps.
- `plan_item_outcomes`: `id` pk; `plan_item_id` integer not_null FK→`plan_items.id` CASCADE; `status` text not_null; `note` text; `recorded_at` text not_null; timestamps.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p healthie-shared migrations_create_all`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add healthie-shared/src
git commit -m "feat: M1 schema migration and in-memory test_db harness

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: All SeaORM entities

**Files:**
- Create: `healthie-shared/src/entities/mod.rs` + one file per table (see structure)
- Modify: `healthie-shared/src/lib.rs` (add `pub mod entities;`)

**Interfaces:**
- Produces: `entities::{profile, concern, concern_tag, goal, protocol, observation, checkin, checkin_response, plan, plan_item, plan_item_outcome}`, each with SeaORM `Model`/`Entity`/`ActiveModel`. Field names/types mirror Task 2's column lists exactly (`Option<T>` for nullable, `String` for text, `i32` for integer, `f64` for real, `bool` is NOT used — `reviewed` is `i32` 0/1).

Entities are mechanical. Full example for `concern.rs`; every other entity repeats the pattern against its Task 2 column list:

- [ ] **Step 1: Write entities**

`entities/concern.rs`:
```rust
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "concerns")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    pub status: String,
    pub narrative: Option<String>,
    pub opened_on: String,
    pub resolved_on: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
```

`entities/mod.rs`:
```rust
pub mod checkin;
pub mod checkin_response;
pub mod concern;
pub mod concern_tag;
pub mod goal;
pub mod observation;
pub mod plan;
pub mod plan_item;
pub mod plan_item_outcome;
pub mod profile;
pub mod protocol;
```

No `Relation` variants needed anywhere in M1 (services join manually by id; simpler than SeaORM relations, matches glovebox).

- [ ] **Step 2: Compile-check with a smoke test**

Append to `test_support.rs` tests:
```rust
    #[tokio::test]
    async fn entities_match_schema() {
        use sea_orm::EntityTrait;
        let db = super::test_db().await;
        // A find() per entity proves column names/types line up with the migration.
        crate::entities::profile::Entity::find().all(&db).await.unwrap();
        crate::entities::concern::Entity::find().all(&db).await.unwrap();
        crate::entities::concern_tag::Entity::find().all(&db).await.unwrap();
        crate::entities::goal::Entity::find().all(&db).await.unwrap();
        crate::entities::protocol::Entity::find().all(&db).await.unwrap();
        crate::entities::observation::Entity::find().all(&db).await.unwrap();
        crate::entities::checkin::Entity::find().all(&db).await.unwrap();
        crate::entities::checkin_response::Entity::find().all(&db).await.unwrap();
        crate::entities::plan::Entity::find().all(&db).await.unwrap();
        crate::entities::plan_item::Entity::find().all(&db).await.unwrap();
        crate::entities::plan_item_outcome::Entity::find().all(&db).await.unwrap();
    }
```

Run: `cargo test -p healthie-shared entities_match_schema`
Expected: PASS (any column-name typo fails here with a decode error).

- [ ] **Step 3: Commit**

```bash
git add healthie-shared/src
git commit -m "feat: SeaORM entities for all M1 tables

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Profile service (singleton upsert)

**Files:**
- Create: `healthie-shared/src/inputs/mod.rs`, `healthie-shared/src/inputs/profile.rs`, `healthie-shared/src/services/mod.rs`, `healthie-shared/src/services/profile.rs`
- Modify: `healthie-shared/src/lib.rs` (add `pub mod inputs; pub mod services;`)

**Interfaces:**
- Consumes: `entities::profile`, `DomainResult`, `clock::now_str`
- Produces: `inputs::profile::UpdateProfile { date_of_birth: Option<Option<String>>, sex: Option<Option<String>>, height_cm: Option<Option<i32>>, notes: Option<Option<String>> }` (Default); `services::profile::get(db) -> DomainResult<Option<Model>>`; `services::profile::upsert(db, UpdateProfile) -> DomainResult<Model>` (creates row id=1 on first call); sex whitelist `["male", "female"]`; `date_of_birth` must parse as `%Y-%m-%d`.

- [ ] **Step 1: Write the failing tests** (inline `#[cfg(test)] mod tests` in `services/profile.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_db;

    #[tokio::test]
    async fn get_returns_none_before_first_upsert() {
        let db = test_db().await;
        assert!(get(&db).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_creates_then_updates_singleton() {
        let db = test_db().await;
        let p = upsert(&db, UpdateProfile {
            date_of_birth: Some(Some("1981-03-02".into())),
            sex: Some(Some("male".into())),
            ..Default::default()
        }).await.unwrap();
        assert_eq!(p.id, 1);
        let p2 = upsert(&db, UpdateProfile {
            height_cm: Some(Some(180)),
            ..Default::default()
        }).await.unwrap();
        assert_eq!(p2.id, 1);
        assert_eq!(p2.date_of_birth.as_deref(), Some("1981-03-02")); // preserved
        assert_eq!(p2.height_cm, Some(180));
    }

    #[tokio::test]
    async fn upsert_rejects_bad_sex_and_bad_dob() {
        let db = test_db().await;
        assert!(matches!(
            upsert(&db, UpdateProfile { sex: Some(Some("yes".into())), ..Default::default() }).await,
            Err(DomainError::BadRequest(_))
        ));
        assert!(matches!(
            upsert(&db, UpdateProfile { date_of_birth: Some(Some("03/02/1981".into())), ..Default::default() }).await,
            Err(DomainError::Invalid { .. })
        ));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p healthie-shared services::profile`
Expected: compile FAIL (module missing).

- [ ] **Step 3: Implement**

`inputs/profile.rs`:
```rust
/// Partial update; outer Option = "was the field sent", inner = "set vs clear".
#[derive(Debug, Default)]
pub struct UpdateProfile {
    pub date_of_birth: Option<Option<String>>,
    pub sex: Option<Option<String>>,
    pub height_cm: Option<Option<i32>>,
    pub notes: Option<Option<String>>,
}
```

`services/profile.rs`:
```rust
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait, EntityTrait};

use crate::{
    clock::now_str,
    entities::profile,
    error::{DomainError, DomainResult},
    inputs::profile::UpdateProfile,
};

pub const VALID_SEXES: [&str; 2] = ["male", "female"];

pub async fn get(db: &impl ConnectionTrait) -> DomainResult<Option<profile::Model>> {
    Ok(profile::Entity::find_by_id(1).one(db).await?)
}

pub async fn upsert(db: &impl ConnectionTrait, input: UpdateProfile) -> DomainResult<profile::Model> {
    if let Some(Some(sex)) = &input.sex {
        if !VALID_SEXES.contains(&sex.as_str()) {
            return Err(DomainError::BadRequest(format!(
                "Invalid sex '{sex}'. Must be one of: {}",
                VALID_SEXES.join(", ")
            )));
        }
    }
    if let Some(Some(dob)) = &input.date_of_birth {
        if chrono::NaiveDate::parse_from_str(dob, "%Y-%m-%d").is_err() {
            return Err(DomainError::invalid("date_of_birth", "must be YYYY-MM-DD"));
        }
    }

    let existing = get(db).await?;
    let mut active: profile::ActiveModel = match existing {
        Some(m) => m.into(),
        None => profile::ActiveModel { id: Set(1), ..Default::default() },
    };
    if let Some(v) = input.date_of_birth { active.date_of_birth = Set(v); }
    if let Some(v) = input.sex { active.sex = Set(v); }
    if let Some(v) = input.height_cm { active.height_cm = Set(v); }
    if let Some(v) = input.notes { active.notes = Set(v); }
    active.updated_at = Set(now_str());
    // insert needs created_at explicitly (ActiveModel bypasses the column default)
    if active.created_at.is_not_set() { active.created_at = Set(now_str()); }
    Ok(active.save(db).await?.try_into_model()?)
}
```

(If `try_into_model` needs it, import `sea_orm::TryIntoModel`.)

`inputs/mod.rs`: `pub mod profile;` — `services/mod.rs`: `pub mod profile;`

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test -p healthie-shared services::profile`
Expected: 3 PASS.

- [ ] **Step 5: Commit**

```bash
git add healthie-shared/src
git commit -m "feat: profile singleton service with validation

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Concern service (open, status, tags, list)

**Files:**
- Create: `healthie-shared/src/inputs/concern.rs`, `healthie-shared/src/services/concern.rs`
- Modify: `inputs/mod.rs`, `services/mod.rs`

**Interfaces:**
- Consumes: `entities::{concern, concern_tag}`, Task 1 core
- Produces:
  - `inputs::concern::NewConcern { name: String, narrative: Option<String>, tags: Vec<String>, opened_on: Option<String> }` (opened_on defaults to today)
  - `services::concern::VALID_STATUSES: [&str; 3] = ["active", "monitoring", "resolved"]`
  - `services::concern::VALID_TAGS: [&str; 10] = ["musculoskeletal", "neurological", "mental-health", "cardiovascular", "metabolic", "nutrition", "preventive", "immune", "sleep", "general"]`
  - `services::concern::ConcernWithTags { pub concern: concern::Model, pub tags: Vec<String> }` (derives `Debug, Serialize`)
  - `open(db, NewConcern) -> DomainResult<ConcernWithTags>`
  - `require(db, id) -> DomainResult<concern::Model>`
  - `update_status(db, id, status: &str, note: Option<String>) -> DomainResult<concern::Model>` — appends note to narrative with a dated line; sets `resolved_on` = today when status == "resolved", clears it otherwise
  - `list_active(db) -> DomainResult<Vec<ConcernWithTags>>` — status != "resolved", tags loaded per concern

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_db;

    async fn seed(db: &sea_orm::DatabaseConnection) -> ConcernWithTags {
        open(db, NewConcern {
            name: "Bad back".into(),
            narrative: Some("L4/L5 disc".into()),
            tags: vec!["musculoskeletal".into(), "neurological".into()],
            opened_on: None,
        }).await.unwrap()
    }

    #[tokio::test]
    async fn open_stores_concern_with_tags() {
        let db = test_db().await;
        let c = seed(&db).await;
        assert_eq!(c.concern.status, "active");
        assert_eq!(c.tags, vec!["musculoskeletal", "neurological"]);
    }

    #[tokio::test]
    async fn open_rejects_unknown_tag() {
        let db = test_db().await;
        let res = open(&db, NewConcern {
            name: "X".into(), narrative: None,
            tags: vec!["chiropractics".into()], opened_on: None,
        }).await;
        assert!(matches!(res, Err(DomainError::BadRequest(_))));
    }

    #[tokio::test]
    async fn update_status_resolves_and_appends_note() {
        let db = test_db().await;
        let c = seed(&db).await;
        let updated = update_status(&db, c.concern.id, "resolved", Some("PT finished".into()))
            .await.unwrap();
        assert_eq!(updated.status, "resolved");
        assert!(updated.resolved_on.is_some());
        assert!(updated.narrative.unwrap().contains("PT finished"));
        assert!(list_active(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_status_rejects_unknown_status_and_missing_id() {
        let db = test_db().await;
        let c = seed(&db).await;
        assert!(matches!(
            update_status(&db, c.concern.id, "cured", None).await,
            Err(DomainError::BadRequest(_))
        ));
        assert!(matches!(
            update_status(&db, 999, "active", None).await,
            Err(DomainError::NotFound(_))
        ));
    }
}
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p healthie-shared services::concern`

- [ ] **Step 3: Implement**

`inputs/concern.rs`:
```rust
#[derive(Debug)]
pub struct NewConcern {
    pub name: String,
    pub narrative: Option<String>,
    pub tags: Vec<String>,
    /// YYYY-MM-DD; defaults to today.
    pub opened_on: Option<String>,
}
```

`services/concern.rs`:
```rust
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};
use serde::Serialize;

use crate::{
    clock::{now_str, today_str},
    entities::{concern, concern_tag},
    error::{DomainError, DomainResult},
    inputs::concern::NewConcern,
};

pub const VALID_STATUSES: [&str; 3] = ["active", "monitoring", "resolved"];
pub const VALID_TAGS: [&str; 10] = [
    "musculoskeletal", "neurological", "mental-health", "cardiovascular", "metabolic",
    "nutrition", "preventive", "immune", "sleep", "general",
];

#[derive(Debug, Serialize)]
pub struct ConcernWithTags {
    pub concern: concern::Model,
    pub tags: Vec<String>,
}

fn validate_tags(tags: &[String]) -> DomainResult<()> {
    for tag in tags {
        if !VALID_TAGS.contains(&tag.as_str()) {
            return Err(DomainError::BadRequest(format!(
                "Invalid tag '{tag}'. Must be one of: {}",
                VALID_TAGS.join(", ")
            )));
        }
    }
    Ok(())
}

pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<concern::Model> {
    concern::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Concern {id} not found")))
}

pub async fn open(db: &impl ConnectionTrait, input: NewConcern) -> DomainResult<ConcernWithTags> {
    if input.name.trim().is_empty() {
        return Err(DomainError::invalid("name", "must not be empty"));
    }
    validate_tags(&input.tags)?;
    let model = concern::ActiveModel {
        name: Set(input.name),
        status: Set("active".into()),
        narrative: Set(input.narrative),
        opened_on: Set(input.opened_on.unwrap_or_else(today_str)),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?;
    let mut tags = Vec::new();
    for tag in input.tags {
        concern_tag::ActiveModel {
            concern_id: Set(model.id),
            tag: Set(tag.clone()),
            created_at: Set(now_str()),
            updated_at: Set(now_str()),
            ..Default::default()
        }
        .insert(db)
        .await?;
        tags.push(tag);
    }
    Ok(ConcernWithTags { concern: model, tags })
}

pub async fn update_status(
    db: &impl ConnectionTrait,
    id: i32,
    status: &str,
    note: Option<String>,
) -> DomainResult<concern::Model> {
    if !VALID_STATUSES.contains(&status) {
        return Err(DomainError::BadRequest(format!(
            "Invalid status '{status}'. Must be one of: {}",
            VALID_STATUSES.join(", ")
        )));
    }
    let existing = require(db, id).await?;
    let mut narrative = existing.narrative.clone().unwrap_or_default();
    if let Some(note) = note {
        if !narrative.is_empty() {
            narrative.push('\n');
        }
        narrative.push_str(&format!("[{}] {note}", today_str()));
    }
    let mut active: concern::ActiveModel = existing.into();
    active.status = Set(status.to_string());
    active.narrative = Set(if narrative.is_empty() { None } else { Some(narrative) });
    active.resolved_on = Set(if status == "resolved" { Some(today_str()) } else { None });
    active.updated_at = Set(now_str());
    Ok(active.update(db).await?)
}

pub async fn tags_for(db: &impl ConnectionTrait, concern_id: i32) -> DomainResult<Vec<String>> {
    Ok(concern_tag::Entity::find()
        .filter(concern_tag::Column::ConcernId.eq(concern_id))
        .order_by_asc(concern_tag::Column::Id)
        .all(db)
        .await?
        .into_iter()
        .map(|t| t.tag)
        .collect())
}

pub async fn list_active(db: &impl ConnectionTrait) -> DomainResult<Vec<ConcernWithTags>> {
    let concerns = concern::Entity::find()
        .filter(concern::Column::Status.ne("resolved"))
        .order_by_asc(concern::Column::Id)
        .all(db)
        .await?;
    let mut out = Vec::with_capacity(concerns.len());
    for c in concerns {
        let tags = tags_for(db, c.id).await?;
        out.push(ConcernWithTags { concern: c, tags });
    }
    Ok(out)
}
```

- [ ] **Step 4: Run tests, verify 4 PASS**

Run: `cargo test -p healthie-shared services::concern`

- [ ] **Step 5: Commit**

```bash
git add healthie-shared/src
git commit -m "feat: concern service with tags and status lifecycle

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Goal service

**Files:**
- Create: `healthie-shared/src/inputs/goal.rs`, `healthie-shared/src/services/goal.rs`
- Modify: `inputs/mod.rs`, `services/mod.rs`

**Interfaces:**
- Consumes: `entities::goal`, `services::concern::require`
- Produces:
  - `inputs::goal::NewGoal { concern_id: Option<i32>, title: String, description: Option<String>, metric_kind: Option<String>, comparison: Option<String>, target_value: Option<f64>, target_high: Option<f64>, target_date: Option<String> }`
  - `services::goal::VALID_STATUSES: [&str; 4] = ["active", "achieved", "abandoned", "paused"]`
  - `services::goal::VALID_COMPARISONS: [&str; 3] = ["at-most", "at-least", "range"]`
  - `set(db, NewGoal) -> DomainResult<goal::Model>`, `require(db, id)`, `update_status(db, id, status) -> DomainResult<goal::Model>`, `list_active(db) -> DomainResult<Vec<goal::Model>>`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{inputs::concern::NewConcern, services::concern, test_support::test_db};

    #[tokio::test]
    async fn set_creates_metric_goal_under_concern() {
        let db = test_db().await;
        let c = concern::open(&db, NewConcern {
            name: "Weight".into(), narrative: None, tags: vec![], opened_on: None,
        }).await.unwrap();
        let g = set(&db, NewGoal {
            concern_id: Some(c.concern.id),
            title: "Reach 175 lbs".into(),
            description: None,
            metric_kind: Some("body_mass_lbs".into()),
            comparison: Some("at-most".into()),
            target_value: Some(175.0),
            target_high: None,
            target_date: Some("2026-12-31".into()),
        }).await.unwrap();
        assert_eq!(g.status, "active");
        assert_eq!(list_active(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn set_validates_comparison_range_and_concern() {
        let db = test_db().await;
        // range needs target_high
        let res = set(&db, NewGoal {
            concern_id: None, title: "Sleep 7-8h".into(), description: None,
            metric_kind: Some("sleep_hours".into()), comparison: Some("range".into()),
            target_value: Some(7.0), target_high: None, target_date: None,
        }).await;
        assert!(matches!(res, Err(DomainError::Invalid { .. })));
        // unknown comparison
        let res = set(&db, NewGoal {
            concern_id: None, title: "X".into(), description: None,
            metric_kind: Some("x".into()), comparison: Some("about".into()),
            target_value: Some(1.0), target_high: None, target_date: None,
        }).await;
        assert!(matches!(res, Err(DomainError::BadRequest(_))));
        // dangling concern id
        let res = set(&db, NewGoal {
            concern_id: Some(999), title: "X".into(), description: None,
            metric_kind: None, comparison: None, target_value: None,
            target_high: None, target_date: None,
        }).await;
        assert!(matches!(res, Err(DomainError::NotFound(_))));
    }

    #[tokio::test]
    async fn achieved_goal_leaves_active_list() {
        let db = test_db().await;
        let g = set(&db, NewGoal {
            concern_id: None, title: "Qualitative: no panic attacks".into(),
            description: None, metric_kind: None, comparison: None,
            target_value: None, target_high: None, target_date: None,
        }).await.unwrap();
        update_status(&db, g.id, "achieved").await.unwrap();
        assert!(list_active(&db).await.unwrap().is_empty());
    }
}
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p healthie-shared services::goal`

- [ ] **Step 3: Implement**

`inputs/goal.rs`:
```rust
#[derive(Debug)]
pub struct NewGoal {
    pub concern_id: Option<i32>,
    pub title: String,
    pub description: Option<String>,
    /// e.g. "body_mass_lbs", "resting_heart_rate" — free text until M2 metrics land.
    pub metric_kind: Option<String>,
    pub comparison: Option<String>,
    pub target_value: Option<f64>,
    pub target_high: Option<f64>,
    /// YYYY-MM-DD
    pub target_date: Option<String>,
}
```

`services/goal.rs`:
```rust
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    clock::now_str,
    entities::goal,
    error::{DomainError, DomainResult},
    inputs::goal::NewGoal,
    services::concern,
};

pub const VALID_STATUSES: [&str; 4] = ["active", "achieved", "abandoned", "paused"];
pub const VALID_COMPARISONS: [&str; 3] = ["at-most", "at-least", "range"];

pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<goal::Model> {
    goal::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Goal {id} not found")))
}

pub async fn set(db: &impl ConnectionTrait, input: NewGoal) -> DomainResult<goal::Model> {
    if input.title.trim().is_empty() {
        return Err(DomainError::invalid("title", "must not be empty"));
    }
    if let Some(cid) = input.concern_id {
        concern::require(db, cid).await?;
    }
    if let Some(cmp) = &input.comparison {
        if !VALID_COMPARISONS.contains(&cmp.as_str()) {
            return Err(DomainError::BadRequest(format!(
                "Invalid comparison '{cmp}'. Must be one of: {}",
                VALID_COMPARISONS.join(", ")
            )));
        }
        if input.target_value.is_none() {
            return Err(DomainError::invalid("target_value", "required when comparison is set"));
        }
        if cmp == "range" && input.target_high.is_none() {
            return Err(DomainError::invalid("target_high", "required when comparison is 'range'"));
        }
    }
    Ok(goal::ActiveModel {
        concern_id: Set(input.concern_id),
        title: Set(input.title),
        description: Set(input.description),
        metric_kind: Set(input.metric_kind),
        comparison: Set(input.comparison),
        target_value: Set(input.target_value),
        target_high: Set(input.target_high),
        target_date: Set(input.target_date),
        status: Set("active".into()),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn update_status(db: &impl ConnectionTrait, id: i32, status: &str) -> DomainResult<goal::Model> {
    if !VALID_STATUSES.contains(&status) {
        return Err(DomainError::BadRequest(format!(
            "Invalid status '{status}'. Must be one of: {}",
            VALID_STATUSES.join(", ")
        )));
    }
    let mut active: goal::ActiveModel = require(db, id).await?.into();
    active.status = Set(status.to_string());
    active.updated_at = Set(now_str());
    Ok(active.update(db).await?)
}

pub async fn list_active(db: &impl ConnectionTrait) -> DomainResult<Vec<goal::Model>> {
    Ok(goal::Entity::find()
        .filter(goal::Column::Status.eq("active"))
        .order_by_asc(goal::Column::Id)
        .all(db)
        .await?)
}
```

- [ ] **Step 4: Run tests, verify 3 PASS**, then **Step 5: Commit**

```bash
git add healthie-shared/src
git commit -m "feat: goal service with target validation

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: Protocol service (the keto guardrail)

**Files:**
- Create: `healthie-shared/src/inputs/protocol.rs`, `healthie-shared/src/services/protocol.rs`
- Modify: `inputs/mod.rs`, `services/mod.rs`

**Interfaces:**
- Consumes: `entities::protocol`, `services::{concern, goal}::require`
- Produces:
  - `inputs::protocol::NewProtocol { concern_id: Option<i32>, goal_id: Option<i32>, name: String, kind: String, purpose: Option<String>, schedule: Option<String>, started_on: Option<String>, review_by: Option<String> }`
  - `inputs::protocol::ProtocolOutcome { verdict: String, rationale: String, ended_on: Option<String> }`
  - `services::protocol::VALID_KINDS: [&str; 6] = ["diet", "exercise", "supplement", "therapy", "screening", "habit"]`
  - `services::protocol::VALID_VERDICTS: [&str; 4] = ["worked", "didnt-work", "mixed", "inconclusive"]`
  - `start(db, NewProtocol) -> DomainResult<protocol::Model>`, `require(db, id)`
  - `record_outcome(db, id, ProtocolOutcome) -> DomainResult<protocol::Model>` — sets `ended_on` (default today), verdict + rationale (rationale mandatory: this is the historical record)
  - `list_active(db) -> DomainResult<Vec<protocol::Model>>` (ended_on IS NULL)
  - `history(db) -> DomainResult<Vec<protocol::Model>>` (all, newest first — feeds `get_protocol_history` MCP tool)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_db;

    async fn keto(db: &sea_orm::DatabaseConnection) -> protocol::Model {
        start(db, NewProtocol {
            concern_id: None, goal_id: None,
            name: "Keto diet".into(), kind: "diet".into(),
            purpose: Some("lose weight".into()), schedule: None,
            started_on: Some("2026-05-01".into()), review_by: None,
        }).await.unwrap()
    }

    #[tokio::test]
    async fn start_rejects_unknown_kind() {
        let db = test_db().await;
        let res = start(&db, NewProtocol {
            concern_id: None, goal_id: None, name: "X".into(), kind: "regimen".into(),
            purpose: None, schedule: None, started_on: None, review_by: None,
        }).await;
        assert!(matches!(res, Err(DomainError::BadRequest(_))));
    }

    #[tokio::test]
    async fn outcome_requires_rationale_and_ends_protocol() {
        let db = test_db().await;
        let p = keto(&db).await;
        assert!(matches!(
            record_outcome(&db, p.id, ProtocolOutcome {
                verdict: "mixed".into(), rationale: "  ".into(), ended_on: None,
            }).await,
            Err(DomainError::Invalid { .. })
        ));
        let done = record_outcome(&db, p.id, ProtocolOutcome {
            verdict: "mixed".into(),
            rationale: "weight down but LDL up".into(),
            ended_on: None,
        }).await.unwrap();
        assert!(done.ended_on.is_some());
        assert_eq!(done.verdict.as_deref(), Some("mixed"));
        assert!(list_active(&db).await.unwrap().is_empty());
        assert_eq!(history(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn outcome_rejects_double_verdict() {
        let db = test_db().await;
        let p = keto(&db).await;
        let outcome = ProtocolOutcome {
            verdict: "worked".into(), rationale: "fine".into(), ended_on: None,
        };
        record_outcome(&db, p.id, outcome.clone()).await.unwrap();
        assert!(matches!(
            record_outcome(&db, p.id, outcome).await,
            Err(DomainError::BadRequest(_))
        ));
    }
}
```

(Derive `Clone` on `ProtocolOutcome` for the test.)

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p healthie-shared services::protocol`

- [ ] **Step 3: Implement**

`inputs/protocol.rs`:
```rust
#[derive(Debug)]
pub struct NewProtocol {
    pub concern_id: Option<i32>,
    pub goal_id: Option<i32>,
    pub name: String,
    pub kind: String,
    pub purpose: Option<String>,
    /// Freetext, e.g. "400mg with dinner, daily".
    pub schedule: Option<String>,
    /// YYYY-MM-DD; defaults to today.
    pub started_on: Option<String>,
    /// YYYY-MM-DD; when to re-evaluate whether this is still needed.
    pub review_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProtocolOutcome {
    pub verdict: String,
    /// Mandatory: WHY — this is the permanent record that prevents re-suggesting.
    pub rationale: String,
    /// YYYY-MM-DD; defaults to today.
    pub ended_on: Option<String>,
}
```

`services/protocol.rs`:
```rust
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    clock::{now_str, today_str},
    entities::protocol,
    error::{DomainError, DomainResult},
    inputs::protocol::{NewProtocol, ProtocolOutcome},
    services::{concern, goal},
};

pub const VALID_KINDS: [&str; 6] = ["diet", "exercise", "supplement", "therapy", "screening", "habit"];
pub const VALID_VERDICTS: [&str; 4] = ["worked", "didnt-work", "mixed", "inconclusive"];

pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<protocol::Model> {
    protocol::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Protocol {id} not found")))
}

pub async fn start(db: &impl ConnectionTrait, input: NewProtocol) -> DomainResult<protocol::Model> {
    if input.name.trim().is_empty() {
        return Err(DomainError::invalid("name", "must not be empty"));
    }
    if !VALID_KINDS.contains(&input.kind.as_str()) {
        return Err(DomainError::BadRequest(format!(
            "Invalid kind '{}'. Must be one of: {}",
            input.kind,
            VALID_KINDS.join(", ")
        )));
    }
    if let Some(cid) = input.concern_id {
        concern::require(db, cid).await?;
    }
    if let Some(gid) = input.goal_id {
        goal::require(db, gid).await?;
    }
    Ok(protocol::ActiveModel {
        concern_id: Set(input.concern_id),
        goal_id: Set(input.goal_id),
        name: Set(input.name),
        kind: Set(input.kind),
        purpose: Set(input.purpose),
        schedule: Set(input.schedule),
        started_on: Set(input.started_on.unwrap_or_else(today_str)),
        review_by: Set(input.review_by),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn record_outcome(
    db: &impl ConnectionTrait,
    id: i32,
    outcome: ProtocolOutcome,
) -> DomainResult<protocol::Model> {
    if !VALID_VERDICTS.contains(&outcome.verdict.as_str()) {
        return Err(DomainError::BadRequest(format!(
            "Invalid verdict '{}'. Must be one of: {}",
            outcome.verdict,
            VALID_VERDICTS.join(", ")
        )));
    }
    if outcome.rationale.trim().is_empty() {
        return Err(DomainError::invalid(
            "rationale",
            "required — the WHY is the permanent record",
        ));
    }
    let existing = require(db, id).await?;
    if existing.verdict.is_some() {
        return Err(DomainError::BadRequest(format!(
            "Protocol {id} already has a verdict; start a new protocol instead of rewriting history"
        )));
    }
    let mut active: protocol::ActiveModel = existing.into();
    active.verdict = Set(Some(outcome.verdict));
    active.verdict_rationale = Set(Some(outcome.rationale));
    active.ended_on = Set(Some(outcome.ended_on.unwrap_or_else(today_str)));
    active.updated_at = Set(now_str());
    Ok(active.update(db).await?)
}

pub async fn list_active(db: &impl ConnectionTrait) -> DomainResult<Vec<protocol::Model>> {
    Ok(protocol::Entity::find()
        .filter(protocol::Column::EndedOn.is_null())
        .order_by_asc(protocol::Column::Id)
        .all(db)
        .await?)
}

pub async fn history(db: &impl ConnectionTrait) -> DomainResult<Vec<protocol::Model>> {
    Ok(protocol::Entity::find()
        .order_by_desc(protocol::Column::StartedOn)
        .all(db)
        .await?)
}
```

- [ ] **Step 4: Run tests (3 PASS)**, then **Step 5: Commit**

```bash
git add healthie-shared/src
git commit -m "feat: protocol service with mandatory outcome rationale

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: Observation service

**Files:**
- Create: `healthie-shared/src/inputs/observation.rs`, `healthie-shared/src/services/observation.rs`
- Modify: `inputs/mod.rs`, `services/mod.rs`

**Interfaces:**
- Consumes: `entities::observation`, `services::concern::require`
- Produces:
  - `inputs::observation::NewObservation { origin: String, kind: String, body: String, severity: Option<i32>, concern_id: Option<i32>, occurred_at: Option<String> }`
  - `services::observation::VALID_ORIGINS: [&str; 3] = ["self", "ai", "rules"]`
  - `services::observation::VALID_KINDS: [&str; 2] = ["note", "symptom"]`
  - `log(db, NewObservation) -> DomainResult<observation::Model>` — severity only valid 1..=10; `ai`/`rules` origins start `reviewed = 0`, `self` starts `reviewed = 1` (nothing to review)
  - `pending_review(db) -> DomainResult<Vec<observation::Model>>` (reviewed = 0)
  - `mark_reviewed(db, id) -> DomainResult<observation::Model>`
  - `recent(db, since: &str) -> DomainResult<Vec<observation::Model>>` (occurred_at >= since, newest first)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_db;

    fn spasm(origin: &str) -> NewObservation {
        NewObservation {
            origin: origin.into(),
            kind: "symptom".into(),
            body: "Back spasm getting out of the car".into(),
            severity: Some(6),
            concern_id: None,
            occurred_at: None,
        }
    }

    #[tokio::test]
    async fn self_observations_need_no_review_but_ai_do() {
        let db = test_db().await;
        log(&db, spasm("self")).await.unwrap();
        let ai = log(&db, NewObservation {
            origin: "ai".into(), kind: "note".into(),
            body: "Resting HR elevated since Tuesday".into(),
            severity: None, concern_id: None, occurred_at: None,
        }).await.unwrap();
        let pending = pending_review(&db).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, ai.id);
        mark_reviewed(&db, ai.id).await.unwrap();
        assert!(pending_review(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn log_validates_origin_kind_severity() {
        let db = test_db().await;
        assert!(matches!(log(&db, spasm("claude")).await, Err(DomainError::BadRequest(_))));
        let mut bad_kind = spasm("self");
        bad_kind.kind = "feeling".into();
        assert!(matches!(log(&db, bad_kind).await, Err(DomainError::BadRequest(_))));
        let mut bad_sev = spasm("self");
        bad_sev.severity = Some(11);
        assert!(matches!(log(&db, bad_sev).await, Err(DomainError::Invalid { .. })));
    }

    #[tokio::test]
    async fn recent_filters_by_date() {
        let db = test_db().await;
        let mut old = spasm("self");
        old.occurred_at = Some("2026-01-01 08:00:00".into());
        log(&db, old).await.unwrap();
        log(&db, spasm("self")).await.unwrap(); // now
        assert_eq!(recent(&db, "2026-06-01").await.unwrap().len(), 1);
    }
}
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p healthie-shared services::observation`

- [ ] **Step 3: Implement**

`inputs/observation.rs`:
```rust
#[derive(Debug)]
pub struct NewObservation {
    /// "self" (you felt it), "ai" (Claude spotted it in data), "rules" (deterministic flag).
    pub origin: String,
    /// "note" or "symptom".
    pub kind: String,
    pub body: String,
    /// 1-10, symptoms only in practice.
    pub severity: Option<i32>,
    pub concern_id: Option<i32>,
    /// "%Y-%m-%d %H:%M:%S"; defaults to now.
    pub occurred_at: Option<String>,
}
```

`services/observation.rs`:
```rust
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    clock::now_str,
    entities::observation,
    error::{DomainError, DomainResult},
    inputs::observation::NewObservation,
    services::concern,
};

pub const VALID_ORIGINS: [&str; 3] = ["self", "ai", "rules"];
pub const VALID_KINDS: [&str; 2] = ["note", "symptom"];

pub async fn log(db: &impl ConnectionTrait, input: NewObservation) -> DomainResult<observation::Model> {
    if !VALID_ORIGINS.contains(&input.origin.as_str()) {
        return Err(DomainError::BadRequest(format!(
            "Invalid origin '{}'. Must be one of: {}",
            input.origin,
            VALID_ORIGINS.join(", ")
        )));
    }
    if !VALID_KINDS.contains(&input.kind.as_str()) {
        return Err(DomainError::BadRequest(format!(
            "Invalid kind '{}'. Must be one of: {}",
            input.kind,
            VALID_KINDS.join(", ")
        )));
    }
    if input.body.trim().is_empty() {
        return Err(DomainError::invalid("body", "must not be empty"));
    }
    if let Some(s) = input.severity {
        if !(1..=10).contains(&s) {
            return Err(DomainError::invalid("severity", "must be 1-10"));
        }
    }
    if let Some(cid) = input.concern_id {
        concern::require(db, cid).await?;
    }
    let reviewed = i32::from(input.origin == "self");
    Ok(observation::ActiveModel {
        occurred_at: Set(input.occurred_at.unwrap_or_else(now_str)),
        origin: Set(input.origin),
        kind: Set(input.kind),
        body: Set(input.body),
        severity: Set(input.severity),
        concern_id: Set(input.concern_id),
        reviewed: Set(reviewed),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn pending_review(db: &impl ConnectionTrait) -> DomainResult<Vec<observation::Model>> {
    Ok(observation::Entity::find()
        .filter(observation::Column::Reviewed.eq(0))
        .order_by_asc(observation::Column::OccurredAt)
        .all(db)
        .await?)
}

pub async fn mark_reviewed(db: &impl ConnectionTrait, id: i32) -> DomainResult<observation::Model> {
    let existing = observation::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Observation {id} not found")))?;
    let mut active: observation::ActiveModel = existing.into();
    active.reviewed = Set(1);
    active.updated_at = Set(now_str());
    Ok(active.update(db).await?)
}

pub async fn recent(db: &impl ConnectionTrait, since: &str) -> DomainResult<Vec<observation::Model>> {
    Ok(observation::Entity::find()
        .filter(observation::Column::OccurredAt.gte(since))
        .order_by_desc(observation::Column::OccurredAt)
        .all(db)
        .await?)
}
```

- [ ] **Step 4: Run tests (3 PASS)**, then **Step 5: Commit**

```bash
git add healthie-shared/src
git commit -m "feat: observation service with origin-aware review queue

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 9: Checkin service (append-only, resumable)

**Files:**
- Create: `healthie-shared/src/services/checkin.rs`
- Modify: `services/mod.rs`

**Interfaces:**
- Consumes: `entities::{checkin, checkin_response}`, `services::concern::require`
- Produces:
  - `services::checkin::start(db) -> DomainResult<checkin::Model>` — reuses an existing incomplete checkin from today instead of opening a duplicate (resumability per spec error-handling)
  - `record_response(db, checkin_id: i32, question: &str, answer: &str, concern_id: Option<i32>) -> DomainResult<checkin_response::Model>` — rejects writes to a completed checkin
  - `complete(db, checkin_id: i32, summary: &str) -> DomainResult<checkin::Model>`
  - `latest_completed(db) -> DomainResult<Option<(checkin::Model, Vec<checkin_response::Model>)>>`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_db;

    #[tokio::test]
    async fn start_is_idempotent_within_a_day() {
        let db = test_db().await;
        let a = start(&db).await.unwrap();
        let b = start(&db).await.unwrap();
        assert_eq!(a.id, b.id); // resumed, not duplicated
        complete(&db, a.id, "ok week").await.unwrap();
        let c = start(&db).await.unwrap();
        assert_ne!(a.id, c.id); // completed one is closed; new checkin opens
    }

    #[tokio::test]
    async fn responses_append_and_lock_after_complete() {
        let db = test_db().await;
        let ck = start(&db).await.unwrap();
        record_response(&db, ck.id, "How was your week?", "Rough — back flared.", None)
            .await.unwrap();
        record_response(&db, ck.id, "Sleep?", "Bad, kids sick.", None).await.unwrap();
        complete(&db, ck.id, "Back flare, poor sleep.").await.unwrap();
        assert!(matches!(
            record_response(&db, ck.id, "One more?", "no", None).await,
            Err(DomainError::BadRequest(_))
        ));
        let (latest, responses) = latest_completed(&db).await.unwrap().unwrap();
        assert_eq!(latest.id, ck.id);
        assert_eq!(responses.len(), 2);
        assert_eq!(latest.summary.as_deref(), Some("Back flare, poor sleep."));
    }
}
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p healthie-shared services::checkin`

- [ ] **Step 3: Implement**

`services/checkin.rs`:
```rust
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    clock::{now_str, today_str},
    entities::{checkin, checkin_response},
    error::{DomainError, DomainResult},
    services::concern,
};

pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<checkin::Model> {
    checkin::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Checkin {id} not found")))
}

/// Opens today's checkin, or resumes an incomplete one started today.
pub async fn start(db: &impl ConnectionTrait) -> DomainResult<checkin::Model> {
    let today = today_str();
    let open = checkin::Entity::find()
        .filter(checkin::Column::CompletedAt.is_null())
        .filter(checkin::Column::StartedAt.gte(format!("{today} 00:00:00")))
        .one(db)
        .await?;
    if let Some(existing) = open {
        return Ok(existing);
    }
    Ok(checkin::ActiveModel {
        started_at: Set(now_str()),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn record_response(
    db: &impl ConnectionTrait,
    checkin_id: i32,
    question: &str,
    answer: &str,
    concern_id: Option<i32>,
) -> DomainResult<checkin_response::Model> {
    let ck = require(db, checkin_id).await?;
    if ck.completed_at.is_some() {
        return Err(DomainError::BadRequest(format!(
            "Checkin {checkin_id} is already completed; start a new checkin"
        )));
    }
    if answer.trim().is_empty() {
        return Err(DomainError::invalid("answer", "must not be empty"));
    }
    if let Some(cid) = concern_id {
        concern::require(db, cid).await?;
    }
    Ok(checkin_response::ActiveModel {
        checkin_id: Set(checkin_id),
        question: Set(question.to_string()),
        answer: Set(answer.to_string()),
        concern_id: Set(concern_id),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn complete(db: &impl ConnectionTrait, checkin_id: i32, summary: &str) -> DomainResult<checkin::Model> {
    let ck = require(db, checkin_id).await?;
    if ck.completed_at.is_some() {
        return Err(DomainError::BadRequest(format!("Checkin {checkin_id} is already completed")));
    }
    let mut active: checkin::ActiveModel = ck.into();
    active.completed_at = Set(Some(now_str()));
    active.summary = Set(Some(summary.to_string()));
    active.updated_at = Set(now_str());
    Ok(active.update(db).await?)
}

pub async fn latest_completed(
    db: &impl ConnectionTrait,
) -> DomainResult<Option<(checkin::Model, Vec<checkin_response::Model>)>> {
    let latest = checkin::Entity::find()
        .filter(checkin::Column::CompletedAt.is_not_null())
        .order_by_desc(checkin::Column::CompletedAt)
        .one(db)
        .await?;
    let Some(ck) = latest else { return Ok(None) };
    let responses = checkin_response::Entity::find()
        .filter(checkin_response::Column::CheckinId.eq(ck.id))
        .order_by_asc(checkin_response::Column::Id)
        .all(db)
        .await?;
    Ok(Some((ck, responses)))
}
```

- [ ] **Step 4: Run tests (2 PASS)**, then **Step 5: Commit**

```bash
git add healthie-shared/src
git commit -m "feat: append-only resumable checkin service

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 10: Plan service (commit + adherence)

**Files:**
- Create: `healthie-shared/src/inputs/plan.rs`, `healthie-shared/src/services/plan.rs`
- Modify: `inputs/mod.rs`, `services/mod.rs`

**Interfaces:**
- Consumes: `entities::{plan, plan_item, plan_item_outcome}`, `services::checkin::require`
- Produces:
  - `inputs::plan::NewPlan { checkin_id: Option<i32>, starts_on: Option<String>, horizon_days: Option<i32>, guidance: Option<String>, nutrition: Option<String>, items: Vec<NewPlanItem> }`
  - `inputs::plan::NewPlanItem { kind: String, title: String, detail: Option<String>, scheduled_for: Option<String> }`
  - `services::plan::VALID_ITEM_KINDS: [&str; 2] = ["workout", "action"]`
  - `services::plan::VALID_OUTCOME_STATUSES: [&str; 3] = ["done", "skipped", "partial"]`
  - `services::plan::PlanWithItems { pub plan: plan::Model, pub items: Vec<ItemWithOutcome> }`, `ItemWithOutcome { pub item: plan_item::Model, pub outcome: Option<plan_item_outcome::Model> }` (both derive `Debug, Serialize`)
  - `commit(db, NewPlan) -> DomainResult<PlanWithItems>` — at least one item required
  - `record_item_outcome(db, item_id: i32, status: &str, note: Option<String>) -> DomainResult<plan_item_outcome::Model>` — one outcome per item (re-record replaces)
  - `latest(db) -> DomainResult<Option<PlanWithItems>>` (newest by starts_on, items with outcomes)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_db;

    fn pt_plan() -> NewPlan {
        NewPlan {
            checkin_id: None,
            starts_on: None,
            horizon_days: None,
            guidance: Some("Prioritize sleep; back is recovering.".into()),
            nutrition: Some("More fish, no late snacking.".into()),
            items: vec![
                NewPlanItem {
                    kind: "workout".into(), title: "PT: bird-dogs 3x10".into(),
                    detail: None, scheduled_for: Some("2026-07-17".into()),
                },
                NewPlanItem {
                    kind: "action".into(), title: "Book colonoscopy".into(),
                    detail: Some("GP referral first".into()), scheduled_for: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn commit_stores_plan_with_items_and_defaults() {
        let db = test_db().await;
        let p = commit(&db, pt_plan()).await.unwrap();
        assert_eq!(p.plan.horizon_days, 7);
        assert_eq!(p.items.len(), 2);
        assert!(p.items.iter().all(|i| i.outcome.is_none()));
    }

    #[tokio::test]
    async fn commit_rejects_empty_plan_and_bad_kind() {
        let db = test_db().await;
        let mut empty = pt_plan();
        empty.items.clear();
        assert!(matches!(commit(&db, empty).await, Err(DomainError::Invalid { .. })));
        let mut bad = pt_plan();
        bad.items[0].kind = "chore".into();
        assert!(matches!(commit(&db, bad).await, Err(DomainError::BadRequest(_))));
    }

    #[tokio::test]
    async fn outcomes_record_and_replace() {
        let db = test_db().await;
        let p = commit(&db, pt_plan()).await.unwrap();
        let item_id = p.items[0].item.id;
        record_item_outcome(&db, item_id, "skipped", Some("back flared".into())).await.unwrap();
        record_item_outcome(&db, item_id, "partial", None).await.unwrap(); // replaces
        let latest = latest(&db).await.unwrap().unwrap();
        let outcome = latest.items.iter().find(|i| i.item.id == item_id)
            .unwrap().outcome.as_ref().unwrap();
        assert_eq!(outcome.status, "partial");
    }
}
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p healthie-shared services::plan`

- [ ] **Step 3: Implement**

`inputs/plan.rs`:
```rust
#[derive(Debug)]
pub struct NewPlan {
    pub checkin_id: Option<i32>,
    /// YYYY-MM-DD; defaults to today.
    pub starts_on: Option<String>,
    /// Defaults to 7.
    pub horizon_days: Option<i32>,
    pub guidance: Option<String>,
    pub nutrition: Option<String>,
    pub items: Vec<NewPlanItem>,
}

#[derive(Debug)]
pub struct NewPlanItem {
    /// "workout" or "action".
    pub kind: String,
    pub title: String,
    pub detail: Option<String>,
    /// YYYY-MM-DD, for time-bound items Claude pushes to the calendar.
    pub scheduled_for: Option<String>,
}
```

`services/plan.rs`:
```rust
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, ModelTrait,
    QueryFilter, QueryOrder,
};
use serde::Serialize;

use crate::{
    clock::{now_str, today_str},
    entities::{plan, plan_item, plan_item_outcome},
    error::{DomainError, DomainResult},
    inputs::plan::NewPlan,
    services::checkin,
};

pub const VALID_ITEM_KINDS: [&str; 2] = ["workout", "action"];
pub const VALID_OUTCOME_STATUSES: [&str; 3] = ["done", "skipped", "partial"];

#[derive(Debug, Serialize)]
pub struct ItemWithOutcome {
    pub item: plan_item::Model,
    pub outcome: Option<plan_item_outcome::Model>,
}

#[derive(Debug, Serialize)]
pub struct PlanWithItems {
    pub plan: plan::Model,
    pub items: Vec<ItemWithOutcome>,
}

pub async fn commit(db: &impl ConnectionTrait, input: NewPlan) -> DomainResult<PlanWithItems> {
    if input.items.is_empty() {
        return Err(DomainError::invalid("items", "a plan needs at least one item"));
    }
    for item in &input.items {
        if !VALID_ITEM_KINDS.contains(&item.kind.as_str()) {
            return Err(DomainError::BadRequest(format!(
                "Invalid item kind '{}'. Must be one of: {}",
                item.kind,
                VALID_ITEM_KINDS.join(", ")
            )));
        }
        if item.title.trim().is_empty() {
            return Err(DomainError::invalid("items.title", "must not be empty"));
        }
    }
    if let Some(cid) = input.checkin_id {
        checkin::require(db, cid).await?;
    }
    let plan_model = plan::ActiveModel {
        checkin_id: Set(input.checkin_id),
        starts_on: Set(input.starts_on.unwrap_or_else(today_str)),
        horizon_days: Set(input.horizon_days.unwrap_or(7)),
        guidance: Set(input.guidance),
        nutrition: Set(input.nutrition),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?;
    let mut items = Vec::with_capacity(input.items.len());
    for item in input.items {
        let m = plan_item::ActiveModel {
            plan_id: Set(plan_model.id),
            kind: Set(item.kind),
            title: Set(item.title),
            detail: Set(item.detail),
            scheduled_for: Set(item.scheduled_for),
            created_at: Set(now_str()),
            updated_at: Set(now_str()),
            ..Default::default()
        }
        .insert(db)
        .await?;
        items.push(ItemWithOutcome { item: m, outcome: None });
    }
    Ok(PlanWithItems { plan: plan_model, items })
}

pub async fn record_item_outcome(
    db: &impl ConnectionTrait,
    item_id: i32,
    status: &str,
    note: Option<String>,
) -> DomainResult<plan_item_outcome::Model> {
    if !VALID_OUTCOME_STATUSES.contains(&status) {
        return Err(DomainError::BadRequest(format!(
            "Invalid status '{status}'. Must be one of: {}",
            VALID_OUTCOME_STATUSES.join(", ")
        )));
    }
    let item = plan_item::Entity::find_by_id(item_id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Plan item {item_id} not found")))?;
    // one outcome per item: replace any existing
    if let Some(existing) = plan_item_outcome::Entity::find()
        .filter(plan_item_outcome::Column::PlanItemId.eq(item.id))
        .one(db)
        .await?
    {
        existing.delete(db).await?;
    }
    Ok(plan_item_outcome::ActiveModel {
        plan_item_id: Set(item.id),
        status: Set(status.to_string()),
        note: Set(note),
        recorded_at: Set(now_str()),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn latest(db: &impl ConnectionTrait) -> DomainResult<Option<PlanWithItems>> {
    let Some(p) = plan::Entity::find()
        .order_by_desc(plan::Column::StartsOn)
        .order_by_desc(plan::Column::Id)
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    let raw_items = plan_item::Entity::find()
        .filter(plan_item::Column::PlanId.eq(p.id))
        .order_by_asc(plan_item::Column::Id)
        .all(db)
        .await?;
    let mut items = Vec::with_capacity(raw_items.len());
    for item in raw_items {
        let outcome = plan_item_outcome::Entity::find()
            .filter(plan_item_outcome::Column::PlanItemId.eq(item.id))
            .one(db)
            .await?;
        items.push(ItemWithOutcome { item, outcome });
    }
    Ok(Some(PlanWithItems { plan: p, items }))
}
```

- [ ] **Step 4: Run tests (3 PASS)**, then **Step 5: Commit**

```bash
git add healthie-shared/src
git commit -m "feat: plan service with typed items and adherence outcomes

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 11: Briefing assembler — the product

**Files:**
- Create: `healthie-shared/src/services/briefing.rs`
- Modify: `services/mod.rs`

**Interfaces:**
- Consumes: every prior service (`checkin::latest_completed`, `plan::latest`, `concern::list_active`, `goal::list_active`, `protocol::list_active`, `observation::{pending_review, recent}`, `profile::get`)
- Produces (all `Debug + Serialize` — this struct is what `start_checkin` will return over MCP in plan M1b):

```rust
pub struct Briefing {
    pub generated_on: String,                       // YYYY-MM-DD (the `today` arg)
    pub profile: Option<profile::Model>,
    pub days_since_last_checkin: Option<i64>,       // None = first checkin ever
    pub cadence_note: Option<String>,               // set when gap > 10 days
    pub last_checkin: Option<LastCheckin>,          // summary + responses
    pub previous_plan: Option<plan::PlanWithItems>, // with outcomes = accountability
    pub active_concerns: Vec<concern::ConcernWithTags>, // tags ARE the specialist lenses
    pub active_goals: Vec<goal::Model>,
    pub active_protocols: Vec<ProtocolBrief>,       // model + overdue_review flag
    pub observations_pending_review: Vec<observation::Model>,
    pub recent_observations: Vec<observation::Model>, // since last checkin (or 14 days)
}
pub struct LastCheckin { pub summary: Option<String>, pub completed_at: String,
    pub responses: Vec<checkin_response::Model> }
pub struct ProtocolBrief { pub protocol: protocol::Model, pub overdue_review: bool }
```

- `assemble(db, today: &str) -> DomainResult<Briefing>` — `today` injected for testability; callers pass `clock::today_str()`.

Rules: `days_since_last_checkin` = date diff between `today` and last completed checkin's date part; `cadence_note` = `Some("Last checkin was N days ago — widen your questions to cover the whole gap.")` when N > 10; `overdue_review` = `review_by.is_some() && review_by < today`; `recent_observations` window = last completed checkin datetime, else `today - 14 days`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inputs::{concern::NewConcern, observation::NewObservation, plan::{NewPlan, NewPlanItem},
                 protocol::NewProtocol},
        services::{checkin, concern, observation, plan, protocol},
        test_support::test_db,
    };

    #[tokio::test]
    async fn first_ever_briefing_is_empty_but_valid() {
        let db = test_db().await;
        let b = assemble(&db, "2026-07-16").await.unwrap();
        assert!(b.days_since_last_checkin.is_none());
        assert!(b.previous_plan.is_none());
        assert!(b.active_concerns.is_empty());
        // must serialize — it crosses the MCP boundary in M1b
        serde_json::to_string(&b).unwrap();
    }

    #[tokio::test]
    async fn briefing_assembles_full_picture() {
        let db = test_db().await;
        let c = concern::open(&db, NewConcern {
            name: "Bad back".into(), narrative: None,
            tags: vec!["musculoskeletal".into()], opened_on: None,
        }).await.unwrap();
        protocol::start(&db, NewProtocol {
            concern_id: Some(c.concern.id), goal_id: None,
            name: "Magnesium".into(), kind: "supplement".into(),
            purpose: None, schedule: None, started_on: None,
            review_by: Some("2026-07-01".into()), // overdue vs 2026-07-16
        }).await.unwrap();
        observation::log(&db, NewObservation {
            origin: "ai".into(), kind: "note".into(),
            body: "HR trending up".into(), severity: None,
            concern_id: None, occurred_at: None,
        }).await.unwrap();
        let ck = checkin::start(&db).await.unwrap();
        checkin::record_response(&db, ck.id, "Week?", "Fine.", None).await.unwrap();
        checkin::complete(&db, ck.id, "Fine week.").await.unwrap();
        plan::commit(&db, NewPlan {
            checkin_id: Some(ck.id), starts_on: None, horizon_days: None,
            guidance: None, nutrition: None,
            items: vec![NewPlanItem {
                kind: "workout".into(), title: "PT bird-dogs".into(),
                detail: None, scheduled_for: None,
            }],
        }).await.unwrap();

        let b = assemble(&db, "2026-07-16").await.unwrap();
        assert_eq!(b.active_concerns.len(), 1);
        assert_eq!(b.active_concerns[0].tags, vec!["musculoskeletal"]);
        assert!(b.active_protocols[0].overdue_review);
        assert_eq!(b.observations_pending_review.len(), 1);
        assert!(b.previous_plan.is_some());
        assert!(b.last_checkin.is_some());
        assert!(b.days_since_last_checkin.is_some());
    }

    #[tokio::test]
    async fn long_gap_sets_cadence_note() {
        let db = test_db().await;
        let ck = checkin::start(&db).await.unwrap();
        checkin::record_response(&db, ck.id, "Week?", "ok", None).await.unwrap();
        checkin::complete(&db, ck.id, "ok").await.unwrap();
        // completed today; a briefing dated 30 days out sees a 30-day gap
        let future = chrono::Utc::now()
            .checked_add_signed(chrono::Duration::days(30)).unwrap()
            .format("%Y-%m-%d").to_string();
        let b = assemble(&db, &future).await.unwrap();
        assert!(b.cadence_note.is_some());
    }
}
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p healthie-shared services::briefing`

- [ ] **Step 3: Implement**

`services/briefing.rs`:
```rust
use serde::Serialize;

use crate::{
    entities::{checkin_response, observation, profile, protocol},
    error::DomainResult,
    services::{checkin, concern, goal, observation as observation_svc, plan, profile as profile_svc,
               protocol as protocol_svc},
};
use sea_orm::ConnectionTrait;

#[derive(Debug, Serialize)]
pub struct LastCheckin {
    pub summary: Option<String>,
    pub completed_at: String,
    pub responses: Vec<checkin_response::Model>,
}

#[derive(Debug, Serialize)]
pub struct ProtocolBrief {
    pub protocol: protocol::Model,
    pub overdue_review: bool,
}

#[derive(Debug, Serialize)]
pub struct Briefing {
    pub generated_on: String,
    pub profile: Option<profile::Model>,
    pub days_since_last_checkin: Option<i64>,
    pub cadence_note: Option<String>,
    pub last_checkin: Option<LastCheckin>,
    pub previous_plan: Option<plan::PlanWithItems>,
    pub active_concerns: Vec<concern::ConcernWithTags>,
    pub active_goals: Vec<crate::entities::goal::Model>,
    pub active_protocols: Vec<ProtocolBrief>,
    pub observations_pending_review: Vec<observation::Model>,
    pub recent_observations: Vec<observation::Model>,
}

fn date_part(datetime: &str) -> &str {
    datetime.split(' ').next().unwrap_or(datetime)
}

fn days_between(earlier: &str, later: &str) -> Option<i64> {
    let a = chrono::NaiveDate::parse_from_str(earlier, "%Y-%m-%d").ok()?;
    let b = chrono::NaiveDate::parse_from_str(later, "%Y-%m-%d").ok()?;
    Some((b - a).num_days())
}

pub async fn assemble(db: &impl ConnectionTrait, today: &str) -> DomainResult<Briefing> {
    let last = checkin::latest_completed(db).await?;
    let (last_checkin, days_since, since_window) = match last {
        Some((ck, responses)) => {
            let completed_at = ck.completed_at.clone().unwrap_or_else(|| ck.started_at.clone());
            let days = days_between(date_part(&completed_at), today);
            let window = completed_at.clone();
            (
                Some(LastCheckin { summary: ck.summary, completed_at, responses }),
                days,
                window,
            )
        }
        None => {
            let two_weeks_ago = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d")
                .map(|d| (d - chrono::Duration::days(14)).format("%Y-%m-%d").to_string())
                .unwrap_or_else(|_| today.to_string());
            (None, None, two_weeks_ago)
        }
    };

    let cadence_note = days_since.filter(|d| *d > 10).map(|d| {
        format!("Last checkin was {d} days ago — widen your questions to cover the whole gap.")
    });

    let active_protocols = protocol_svc::list_active(db)
        .await?
        .into_iter()
        .map(|p| {
            let overdue_review = p.review_by.as_deref().is_some_and(|r| r < today);
            ProtocolBrief { protocol: p, overdue_review }
        })
        .collect();

    Ok(Briefing {
        generated_on: today.to_string(),
        profile: profile_svc::get(db).await?,
        days_since_last_checkin: days_since,
        cadence_note,
        last_checkin,
        previous_plan: plan::latest(db).await?,
        active_concerns: concern::list_active(db).await?,
        active_goals: goal::list_active(db).await?,
        active_protocols,
        observations_pending_review: observation_svc::pending_review(db).await?,
        recent_observations: observation_svc::recent(db, &since_window).await?,
    })
}
```

- [ ] **Step 4: Run the full suite**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D clippy::pedantic && cargo +nightly fmt --all --check`
Expected: all green — this is the M1a done-gate.

- [ ] **Step 5: Commit**

```bash
git add healthie-shared/src
git commit -m "feat: checkin briefing assembler

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Deliberately NOT in this plan (so nobody "helpfully" adds them)

- **FTS5 / `search`** — lands with the MCP `search` tool when there's content to search.
- **FamilyEvent table** — family context rides in checkin responses for M1; dedicated table when querying demands it.
- **Exercise library, rules layer, DailyMetric/Episode, Documents, ingest endpoint, axum anything** — M2+.
- **`healthie-mcp` crate** — plan M1b, written after this plan executes.

## Self-review notes

- Spec coverage: M1a = "schema + services" half of spec M1; every M1 table has a service task; briefing (Task 11) is the spec's "highest-value test target".
- Type consistency verified: `ConcernWithTags`/`PlanWithItems`/`ItemWithOutcome` names match between producing tasks (5, 10) and the consumer (11); all services take `&impl ConnectionTrait` and return `DomainResult`.
- Migration column lists (Task 2) are the single source of truth; entity fields (Task 3) and service `Set(...)` calls were cross-checked against them.

