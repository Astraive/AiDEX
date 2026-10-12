//! Command handlers, one module per domain.
//!
//! A domain adds `commands/<domain>.rs` with its clap `Subcommand`/`Args` type
//! and `pub fn run(command, output: Output) -> Result<Exit, CliError>`, one
//! `pub mod` line below, one variant in `cli::Commands`, and one dispatch arm.
//! Daemon-backed commands obtain a client with [`daemon::connect`].

pub mod agent;
pub mod approvals;
pub mod context;
pub mod daemon;
pub mod doctor;
pub mod drive;
pub mod emergency;
pub mod events;
pub mod inference;
pub mod mcp;
pub mod model;
pub mod offline;
pub mod omp;
pub mod plugins;
pub mod policy;
pub mod provider;
pub mod session;
pub mod skills;
pub mod tools;
pub mod update;
pub mod vault;
pub mod workspace;

use crate::{
    cli::Commands,
    exit::{CliError, Exit},
    output::Output,
};

pub fn dispatch(command: Option<Commands>, output: Output) -> Result<Exit, CliError> {
    match command {
        None => omp::chat(&omp::SessionOptions::default(), &[]),
        Some(Commands::Chat { options, prompt }) => omp::chat(&options, &prompt),
        Some(Commands::Run { options, prompt }) => omp::run(&options, &prompt),
        Some(Commands::Agent { command }) => agent::run(command, output),
        Some(Commands::Init { drive }) => drive::init(&drive, output),
        Some(Commands::Status { drive }) => drive::status(drive.as_deref(), output),
        Some(Commands::Daemon { command }) => daemon::run(command, output),
        Some(Commands::Events { command }) => events::run(command, output),
        Some(Commands::Doctor) => doctor::run(output),
        Some(Commands::Vault { command }) => vault::run(command, output),
        Some(Commands::Workspace(args)) => workspace::run(args, output),
        Some(Commands::Model(args)) => model::run(args, output),
        Some(Commands::Inference(args)) => inference::run(args, output),
        Some(Commands::Provider(args)) => provider::run(args, output),
        Some(Commands::Offline(args)) => offline::run(args, output),
        Some(Commands::Session(args)) => session::run(args, output),
        Some(Commands::Skills(args)) => skills::run(args, output),
        Some(Commands::Approvals(args)) => approvals::run(args, output),
        Some(Commands::Approve(args)) => approvals::approve(args, output),
        Some(Commands::Deny(args)) => approvals::deny(args, output),
        Some(Commands::Policy(args)) => policy::run(args, output),
        Some(Commands::Tools(args)) => tools::run(args, output),
        Some(Commands::Plugins(args)) => plugins::run(args, output),
        Some(Commands::Mcp(args)) => mcp::run(args, output),
        Some(Commands::Stop(args)) => emergency::stop(args, output),
        Some(Commands::Resume(args)) => emergency::resume(args, output),
        Some(Commands::Index(args)) => context::index::run(args, output),
        Some(Commands::Search(args)) => context::search::run(args, output),
        Some(Commands::Context(args)) => context::pack::run(args, output),
        Some(Commands::Memory(args)) => context::memory::run(args, output),
        Some(Commands::Rules(args)) => context::rules::run(args, output),
        Some(Commands::Update(args)) => update::run(args, output),
    }
}
