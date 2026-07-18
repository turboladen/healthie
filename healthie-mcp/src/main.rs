//! healthie-mcp binary: the interim host for the MCP router until M2's
//! backend exists, plus operator-token management. Runs on the odroid (or
//! anywhere), exposed via Tailscale; bearer auth is mandatory.

use clap::Parser;
use healthie_mcp::config::{Cli, Command, TokenAction};
use healthie_shared::{migration::Migrator, services::mcp_token};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    let db = connect(&cli.db_path).await?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(db, cli.listen).await,
        Command::Token { action } => match action {
            TokenAction::Provision => {
                let issued = mcp_token::provision(&db).await?;
                // The ONE permitted plaintext output, by design: shown once,
                // never stored, never logged.
                println!("MCP bearer token (shown once, store it now):");
                println!("  {}", issued.plaintext);
                println!("fingerprint: {}", issued.fingerprint);
                Ok(())
            }
            TokenAction::Revoke => {
                mcp_token::revoke(&db).await?;
                println!("MCP token revoked. Provision a new one to restore access.");
                Ok(())
            }
        },
    }
}

async fn connect(db_path: &str) -> Result<DatabaseConnection, Box<dyn std::error::Error>> {
    if let Some(parent) = std::path::Path::new(db_path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    // rwc = create the file if missing. WAL for concurrent reads; FKs
    // explicit rather than default-reliant (healthie-38x).
    let mut opts = ConnectOptions::new(format!("sqlite://{db_path}?mode=rwc"));
    opts.sqlx_logging(false).map_sqlx_sqlite_opts(|o| {
        o.journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
    });
    let db = Database::connect(opts).await?;
    Migrator::up(&db, None).await?;
    Ok(db)
}

async fn serve(
    db: DatabaseConnection,
    listen: std::net::SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = healthie_mcp::router(db);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, "healthie-mcp serving (bearer auth required)");
    axum::serve(listener, app).await?;
    Ok(())
}
