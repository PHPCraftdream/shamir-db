use super::create::parse_ttl;
use super::revoke::parse_id;
use std::path::PathBuf;
#[derive(Debug, clap::Args)]
/// Arguments for offline token creation.
pub struct CreateArgs {
    #[arg(long)]
    pub user: String,
    #[arg(long, required = true)]
    pub label: String,
    #[arg(long, value_parser = parse_ttl)]
    pub ttl: Option<u64>,
    #[arg(long)]
    pub read_write: bool,
    #[arg(long,default_value="both",value_parser=["http","ws","both"])]
    pub channels: String,
    #[arg(long = "db")]
    pub databases: Vec<String>,
    #[arg(long)]
    pub out_file: Option<PathBuf>,
}
#[derive(Debug, clap::Args)]
/// Arguments for offline token listing.
pub struct ListArgs {
    #[arg(long)]
    pub user: Option<String>,
}
#[derive(Debug, clap::Subcommand)]
/// Token administration actions.
pub enum TokenAction {
    Create(CreateArgs),
    List(ListArgs),
    Revoke {
        #[arg(value_parser = parse_id)]
        id: String,
    },
    RevokeUser {
        user: String,
    },
    RevokeAll {
        #[arg(long, required = true, action = clap::ArgAction::SetTrue)]
        yes: bool,
    },
}
