mod download;
mod manager;
mod metadata;

use clap::{Parser, Subcommand};
use manager::{InstallOptions, Manager};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "dev",
    version,
    about = "Dev OS Package Manager — install .dpk packages (also supports .tar.gz)"
)]
struct Cli {
    /// Package store (default: $DEV_ROOT or ~/.local/share/devos)
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Install a local file or HTTP(S) URL (.dpk or .tar.gz)
    Install {
        archive: String,
        /// Name for archives without package.json
        #[arg(long)]
        name: Option<String>,
        /// Version for archives without package.json
        #[arg(long)]
        version: Option<String>,
        /// Verify the compressed archive against a trusted SHA-256 digest
        #[arg(long)]
        sha256: Option<String>,
        #[arg(long)]
        replace: bool,
    },
    /// Show installed packages
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show package metadata and installation path
    Info { name: String },
    /// Check installed file hashes, sizes and executable permissions
    Verify { name: Option<String> },
    /// Remove a package and its saved previous version
    Remove { name: String },
    /// Switch back to the saved previous version
    Rollback { name: String },
    /// Execute a command exported by an installed package
    Run {
        command: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Build a .dpk from a directory containing package.json
    Pack {
        directory: PathBuf,
        /// Output filename (default: <name>-<version>.dpk in the current directory)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn execute(cli: Cli) -> manager::Result<i32> {
    if let Commands::Pack { directory, output } = &cli.command {
        let output = match output {
            Some(path) => path.clone(),
            None => manager::default_package_output(directory)?,
        };
        manager::pack(directory, &output)?;
        println!("Created {}", output.display());
        return Ok(0);
    }
    // Download before acquiring the store lock so slow networks do not block readers.
    let downloaded = if let Commands::Install { archive, .. } = &cli.command {
        download::resolve(archive)?
    } else {
        None
    };
    let write = matches!(
        cli.command,
        Commands::Install { .. } | Commands::Remove { .. } | Commands::Rollback { .. }
    );
    let mut manager = Manager::open(cli.root, write)?;
    match cli.command {
        Commands::Install {
            archive,
            name,
            version,
            sha256,
            replace,
        } => {
            let snapshot = manager.install(
                &downloaded
                    .as_ref()
                    .map(|file| file.path().to_path_buf())
                    .unwrap_or_else(|| PathBuf::from(archive)),
                InstallOptions {
                    name,
                    version,
                    sha256,
                    replace,
                },
            )?;
            println!(
                "Installed {} {}",
                snapshot.manifest.name, snapshot.manifest.version
            );
            println!("Path: {}", manager.snapshot_path(&snapshot).display());
            for command in snapshot.manifest.bin.keys() {
                println!("Run: dev run {command}");
            }
        }
        Commands::List { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(manager.packages())?);
            } else if manager.packages().is_empty() {
                println!("No packages installed. Use: dev install <file.dpk>");
            } else {
                for (name, package) in manager.packages() {
                    println!(
                        "{name}\t{}\t{} files\t{} bytes",
                        package.active.manifest.version,
                        package.active.files.len(),
                        package.active.bytes
                    );
                }
            }
        }
        Commands::Info { name } => {
            let package = manager.package(&name)?;
            println!("{}", serde_json::to_string_pretty(package)?);
            println!("Path: {}", manager.snapshot_path(&package.active).display());
            if let Some(release) = package.active.manifest.resolved_release()? {
                println!("Release: {}", serde_json::to_string_pretty(&release)?);
            }
        }
        Commands::Verify { name } => {
            let count = manager.verify(name.as_deref())?;
            println!("Verified {count} package(s): all files intact");
        }
        Commands::Remove { name } => {
            manager.remove(&name)?;
            println!("Removed {name}");
        }
        Commands::Rollback { name } => {
            let version = manager.rollback(&name)?;
            println!("Rolled back {name} to {version}");
        }
        Commands::Run { command, args } => return manager.run(&command, &args),
        Commands::Pack { .. } => unreachable!(),
    }
    Ok(0)
}

fn main() {
    let cli = Cli::parse();
    match execute(cli) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("dev: {error}");
            std::process::exit(1);
        }
    }
}
