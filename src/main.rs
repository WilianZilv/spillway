mod balancer;
mod cluster;
mod config;
mod fs;
mod hooks;
mod loc;
mod master;
mod mp4;
mod node;
mod proto;
mod ram;
mod stats;
mod underlay;
mod web;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "spillway", version, about = "RAM-first FUSE filesystem for recordings, load-balanced to disk across a master/node cluster")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the master: FUSE mount, RAM tier, balancer, thief and web panel.
    Master {
        #[arg(short, long, env = "SPILLWAY_CONFIG", default_value = "/etc/spillway/spillway.toml")]
        config: PathBuf,
    },
    /// Run a node that serves destinations on this host to a remote master.
    Node {
        /// Unique node id (shown in the panel; destinations are assigned to it).
        #[arg(long, env = "SPILLWAY_NODE_ID")]
        id: String,
        /// Master base URL, e.g. http://10.0.0.5:7070
        #[arg(long, env = "SPILLWAY_MASTER")]
        master: String,
        /// Cluster token (master's cluster.token).
        #[arg(long, env = "SPILLWAY_TOKEN")]
        token: String,
        #[arg(long, env = "SPILLWAY_LISTEN", default_value = "0.0.0.0:7071")]
        listen: String,
        /// URL the master uses to reach this node, e.g. http://10.0.0.9:7071
        #[arg(long, env = "SPILLWAY_ADVERTISE")]
        advertise: String,
    },
    /// Print what the MP4 parser sees in a file (debug helper).
    Inspect { file: PathBuf },
}

fn main() -> anyhow::Result<()> {
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,fuser=error".into()))
        .with_target(false)
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().max_blocking_threads(1024).build()?;
    match cli.cmd {
        Cmd::Master { config } => rt.block_on(master::run(config)),
        Cmd::Node { id, master, token, listen, advertise } => {
            anyhow::ensure!(config::valid_id(&id), "invalid node id");
            rt.block_on(node::run(node::NodeArgs { id, master, token, listen, advertise }))
        }
        Cmd::Inspect { file } => {
            let t0 = std::time::Instant::now();
            let r = mp4::parse_file(&file);
            let us = t0.elapsed().as_micros();
            match r {
                Ok(info) => println!("{}", serde_json::to_string_pretty(&info)?),
                Err(e) => println!("parse error: {e}"),
            }
            println!("parsed in {us} µs");
            Ok(())
        }
    }
}
