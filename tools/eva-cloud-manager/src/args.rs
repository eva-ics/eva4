use crate::{cloud_update, deploy, dump, mirror, update};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    #[clap(long = "verbose")]
    pub verbose: bool,
    #[clap(subcommand)]
    pub command: Command,
}

impl Args {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}

#[derive(Subcommand, Clone, Debug)]
pub enum Command {
    #[clap(subcommand)]
    Cloud(CloudCommand),
    #[clap(subcommand)]
    Node(NodeCommand),
}

#[derive(Subcommand, Clone, Debug)]
pub enum CloudCommand {
    Deploy(deploy::Options),
    Undeploy(deploy::Options),
    Update(cloud_update::Options),
}

#[derive(Subcommand, Clone, Debug)]
pub enum NodeCommand {
    Dump(dump::Options),
    Update(update::Options),
    #[clap(name = "mirror-update")]
    MirrorUpdate(mirror::Options),
    #[clap(name = "mirror-set")]
    MirrorSet(mirror::SetOptions),
    Version,
}
