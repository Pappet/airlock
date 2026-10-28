use airlock::cli::Cli;
use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // The hub store does not exist yet (Phase 1); fail loudly instead of
    // pretending a command succeeded.
    anyhow::bail!(
        "`airlock {}` is not implemented yet; see docs/plans/PHASE_PLAN.md",
        cli.command.name()
    )
}
