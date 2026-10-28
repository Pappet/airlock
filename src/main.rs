use airlock::cli::Cli;
use clap::Parser;

fn main() -> anyhow::Result<()> {
    airlock::app::run(Cli::parse())
}
