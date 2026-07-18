//! CLI/env configuration. Port 3005 is arbitrary-but-stable (glovebox owns
//! 3003 on the same box). Binds 0.0.0.0 by default: the server is designed
//! for Tailscale exposure and the bearer middleware is the gate.

use std::net::SocketAddr;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "healthie-mcp", about = "healthie MCP server (M1b)")]
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
    /// Run the MCP server (the default when no subcommand is given).
    Serve,
    /// Manage the operator bearer token.
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum TokenAction {
    /// Create or rotate the token; prints the plaintext ONCE.
    Provision,
    /// Revoke the token; the server rejects all requests until re-provisioned.
    Revoke,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn defaults_are_stable() {
        let cli = Cli::try_parse_from(["healthie-mcp"]).expect("parse");
        assert_eq!(cli.db_path, "data/healthie.db");
        assert_eq!(cli.listen.port(), 3005);
        assert!(cli.command.is_none(), "no subcommand defaults to serve");
    }

    #[test]
    fn token_subcommands_parse() {
        let cli = Cli::try_parse_from(["healthie-mcp", "token", "provision"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Command::Token {
                action: TokenAction::Provision
            })
        ));
        let cli = Cli::try_parse_from(["healthie-mcp", "token", "revoke"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Command::Token {
                action: TokenAction::Revoke
            })
        ));
    }
}
