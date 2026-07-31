//! healthie-backend binary: connect (WAL + FKs + migrate), then serve the API
//! or manage a bearer token. A thin wrapper over the `healthie_backend` library
//! so the same `api::router` is exercised by wire tests.

use clap::Parser;
use healthie_backend::{
    AppState,
    config::{Cli, Command, TokenAction},
};
use healthie_shared::{migration::Migrator, services::auth_token};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        // stderr so `token provision` stdout carries ONLY the token (healthie-pms:
        // fmt() defaults to stdout).
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let db = connect(&cli.db_path).await?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(cli.listen, db).await,
        Command::Token { kind, action } => {
            let kind = kind.into();
            match action {
                TokenAction::Provision => {
                    let issued = auth_token::provision(&db, kind).await?;
                    // The ONE permitted plaintext output: shown once, never stored,
                    // never logged.
                    println!("{}", issued.plaintext);
                    eprintln!("fingerprint: {}", issued.fingerprint);
                }
                TokenAction::Revoke => {
                    auth_token::revoke(&db, kind).await?;
                    eprintln!("revoked {kind:?} token");
                }
            }
            Ok(())
        }
    }
}

async fn connect(db_path: &str) -> anyhow::Result<DatabaseConnection> {
    if let Some(parent) = std::path::Path::new(db_path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    // rwc = create the file if missing. WAL for concurrent reads; FKs explicit
    // rather than default-reliant (healthie-38x).
    let mut opts = ConnectOptions::new(format!("sqlite://{db_path}?mode=rwc"));
    opts.sqlx_logging(false).map_sqlx_sqlite_opts(|o| {
        o.journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
    });
    let db = Database::connect(opts).await?;
    Migrator::up(&db, None).await?;
    Ok(db)
}

async fn serve(listen: std::net::SocketAddr, db: DatabaseConnection) -> anyhow::Result<()> {
    let state = AppState { db: db.clone() };
    let app = healthie_backend::api::router(state, db);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, "healthie-backend listening");
    axum::serve(listener, app).await?;
    Ok(())
}
