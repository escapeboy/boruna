use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "boruna-pkg", about = "Boruna Package Manager")]
struct Cli {
    /// Registry directory (default: ./packages/registry)
    #[arg(long, default_value = "packages/registry")]
    registry: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new package manifest
    Init {
        /// Directory to initialize (default: current dir)
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Add a dependency
    Add {
        /// Package name
        name: String,
        /// Exact version
        version: String,
        /// Package directory
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Remove a dependency
    Remove {
        /// Package name
        name: String,
        /// Package directory
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Resolve dependencies and generate lockfile
    Resolve {
        /// Package directory
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Resolve, verify, and install all packages
    Install {
        /// Package directory
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Publish package to local registry
    Publish {
        /// Package source directory
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Verify all packages in registry
    Verify,
    /// Print dependency tree
    Tree {
        /// Package directory
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Init { dir } => boruna_pkg::cli::cmd_init(&dir),
        Command::Add { name, version, dir } => boruna_pkg::cli::cmd_add(&dir, &name, &version),
        Command::Remove { name, dir } => boruna_pkg::cli::cmd_remove(&dir, &name),
        Command::Resolve { dir } => boruna_pkg::cli::cmd_resolve(&dir, &cli.registry),
        Command::Install { dir } => boruna_pkg::cli::cmd_install(&dir, &cli.registry),
        Command::Publish { dir } => boruna_pkg::cli::cmd_publish(&dir, &cli.registry),
        Command::Verify => boruna_pkg::cli::cmd_verify(&cli.registry),
        Command::Tree { dir } => boruna_pkg::cli::cmd_tree(&dir, &cli.registry),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
