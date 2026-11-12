//! Authentication subcommands share the root CLI's exact clap contract.
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub(in crate::cli) enum AuthCommand {
    /// List login groups with their mode, weights and per-member load.
    Groups,
    /// Show the newest 5-hour, weekly and credit usage each login reported.
    Usage {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name; every login when omitted.
        name: Option<String>,
    },
    /// List configured profiles without starting a provider or creating state.
    List,
    /// Inspect credentials; this does not run an inference turn.
    Status {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
    },
    /// Run the provider's native subscription login in your terminal.
    Login {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
        #[arg(long, help = "Use Codex browser login instead of device login")]
        browser: bool,
        #[arg(
            long,
            help = "Request a new native login for an existing subscription of the same method"
        )]
        reauthenticate: bool,
    },
    /// Reconcile saved process identities and report execution fences without rebooting.
    Recover {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
    },
    /// List active and retired credential generations and their audit metadata.
    Generations {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
    },
    /// Export retired controller history without opening its database or locks.
    HistoryExport {
        #[arg(long)]
        /// New directory for exported retired controller history.
        output: PathBuf,
    },
    /// Remove an unused retired generation and its native history; daemon must be stopped.
    Prune {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
        #[arg(long, value_parser = hive_input::validate::uuid)]
        /// Retired credential generation UUID to prune.
        generation: uuid::Uuid,
    },
    /// Authenticate an isolated credential generation without changing old agents.
    Repair {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
        #[arg(long)]
        /// Fresh controller state directory for the repaired login.
        new_state_dir: Option<PathBuf>,
        #[arg(long, required = true)]
        /// Required confirmation to authenticate a new isolated generation.
        reauthenticate: bool,
        #[arg(long)]
        /// Use Codex browser login instead of device login.
        browser: bool,
    },
}
