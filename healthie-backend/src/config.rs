//! CLI/config for the healthie backend (the single deployed binary). Port 3005
//! is kept from the retired M1b MCP server. Binds 0.0.0.0 by default (designed
//! for Tailscale exposure; the bearer middleware is the gate).

use std::{net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use healthie_shared::entities::auth_token::TokenKind;

#[derive(Debug, Parser)]
#[command(
    name = "healthie-backend",
    about = "healthie backend (API + /ingest/hae + /mcp)"
)]
pub struct Cli {
    /// `SQLite` database file; parent directories are created.
    #[arg(long, env = "HEALTHIE_DB_PATH", default_value = "data/healthie.db")]
    pub db_path: String,

    /// Listen address.
    #[arg(long, env = "HEALTHIE_LISTEN", default_value = "0.0.0.0:3005")]
    pub listen: SocketAddr,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the server (the default when no subcommand is given).
    Serve,
    /// Manage a bearer token.
    Token {
        /// Which token to manage.
        #[arg(long)]
        kind: TokenKindArg,
        #[command(subcommand)]
        action: TokenAction,
    },
    /// One-time backfill of an Apple Health `export.xml` into `daily_metric`.
    ///
    /// BACK UP THE DATABASE FIRST if it already holds data. `(kind, date)`
    /// upserts last-write-wins, so ANY import — with or without
    /// `--replace-range` — irreversibly overwrites whatever occupied the keys
    /// it produces, including rows the live HAE push landed. The command warns
    /// before it starts when the store is non-empty.
    ///
    /// "Idempotent" here means two narrow things and not a third: re-running
    /// the same import converges, and a fix that changes a row's VALUE (a unit
    /// correction) converges. Neither is non-destructive toward rows from
    /// another source. A fix that changes a row's KEY — a sleep-day boundary or
    /// metric-mapping change — additionally strands rows at the old dates or
    /// kinds; those are reported, and `--replace-range` deletes them.
    ImportAppleHealth {
        /// Path to `export.xml` (from the Health app's "Export All Health Data").
        path: PathBuf,

        /// Delete pre-existing rows inside the imported range that this run did
        /// not produce. BACK UP THE DATABASE FILE FIRST — this is not
        /// recoverable.
        ///
        /// Use after changing the sleep-day boundary or a metric mapping, when
        /// the previous import's rows are known to be wrong. Deletes real data,
        /// so it is opt-in: run without it first and read the report.
        ///
        /// A row can look stale for reasons that have nothing to do with a
        /// boundary change — days the live HAE push covered but this export
        /// does not, days where every reading of a kind hit an unconvertible
        /// unit (so no row was produced), and days deleted in the Health app.
        /// Nothing distinguishes those from an earlier import's leftovers, so
        /// read the listed dates before using this.
        ///
        /// Refused when the export is truncated.
        #[arg(long)]
        replace_range: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TokenAction {
    /// Create or rotate the token; prints the plaintext ONCE.
    Provision,
    /// Revoke the token; requests of that kind are rejected until re-provisioned.
    Revoke,
}

/// CLI mirror of the domain [`TokenKind`] — keeps clap out of healthie-shared.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum TokenKindArg {
    Mcp,
    Ingest,
}

impl From<TokenKindArg> for TokenKind {
    fn from(arg: TokenKindArg) -> Self {
        match arg {
            TokenKindArg::Mcp => TokenKind::Mcp,
            TokenKindArg::Ingest => TokenKind::Ingest,
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, TokenAction, TokenKindArg};
    use healthie_shared::entities::auth_token::TokenKind;

    #[test]
    fn defaults_are_stable() {
        let cli = Cli::try_parse_from(["healthie-backend"]).expect("parse");
        assert_eq!(cli.db_path, "data/healthie.db");
        assert_eq!(cli.listen.port(), 3005);
        assert!(cli.command.is_none(), "no subcommand defaults to serve");
    }

    #[test]
    fn token_provision_with_kind_parses() {
        let cli =
            Cli::try_parse_from(["healthie-backend", "token", "--kind", "ingest", "provision"])
                .expect("parse");
        let Some(Command::Token { kind, action }) = cli.command else {
            panic!("expected token subcommand");
        };
        assert!(matches!(kind, TokenKindArg::Ingest));
        assert!(matches!(action, TokenAction::Provision));
        assert_eq!(TokenKind::from(kind), TokenKind::Ingest);
    }

    #[test]
    fn import_apple_health_parses_with_a_path() {
        let cli = Cli::try_parse_from([
            "healthie-backend",
            "import-apple-health",
            "/data/apple_health_export/export.xml",
        ])
        .expect("parse");
        let Some(Command::ImportAppleHealth {
            path,
            replace_range,
        }) = cli.command
        else {
            panic!("expected import-apple-health subcommand");
        };
        assert_eq!(
            path,
            std::path::PathBuf::from("/data/apple_health_export/export.xml")
        );
        assert!(
            !replace_range,
            "deleting existing rows must be opt-in, never the default"
        );
    }

    #[test]
    fn import_apple_health_accepts_replace_range() {
        let cli = Cli::try_parse_from([
            "healthie-backend",
            "import-apple-health",
            "--replace-range",
            "export.xml",
        ])
        .expect("parse");
        let Some(Command::ImportAppleHealth { replace_range, .. }) = cli.command else {
            panic!("expected import-apple-health subcommand");
        };
        assert!(replace_range);
    }

    #[test]
    fn import_apple_health_requires_a_path() {
        assert!(
            Cli::try_parse_from(["healthie-backend", "import-apple-health"]).is_err(),
            "the path is mandatory — importing nothing silently would be worse"
        );
    }

    #[test]
    fn token_revoke_with_mcp_kind_parses() {
        let cli = Cli::try_parse_from(["healthie-backend", "token", "--kind", "mcp", "revoke"])
            .expect("parse");
        let Some(Command::Token { kind, action }) = cli.command else {
            panic!("expected token subcommand");
        };
        assert!(matches!(kind, TokenKindArg::Mcp));
        assert!(matches!(action, TokenAction::Revoke));
        assert_eq!(TokenKind::from(kind), TokenKind::Mcp);
    }
}
