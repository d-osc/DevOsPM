mod desktop;
mod devos;
mod download;
mod manager;
mod metadata;
mod native;
mod progress;
mod shim;
mod update;

use clap::{Parser, Subcommand};
use manager::{InstallOptions, InstallOutcome, Manager};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "dev",
    version,
    about = "Dev OS Package Manager — install .dpk packages, Dev OS extensions and launcher entries"
)]
struct Cli {
    /// Package store (default: $DEV_ROOT or ~/.local/share/devos)
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Dev OS data directory for extensions (default: $XDG_DATA_HOME/dev-os)
    #[arg(long, global = true, name = "DEV_OS_DATA")]
    devos_data: Option<PathBuf>,
    /// Launcher applications directory (default: $XDG_DATA_HOME/applications)
    #[arg(long, global = true)]
    applications: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Install a local file or HTTP(S) URL (.dpk, .dpkx, .tar.gz, .deb, .rpm, or a Dev OS extension)
    #[command(visible_aliases = ["add", "i"])]
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
        /// Do not write a launcher .desktop entry or PATH command shims for this package
        #[arg(long)]
        no_desktop: bool,
    },
    /// Show installed packages and Dev OS extensions
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show package or extension metadata and installation path
    Info { name: String },
    /// Check installed file hashes, sizes and executable permissions
    Verify { name: Option<String> },
    /// Remove a package or Dev OS extension and its saved previous version
    #[command(visible_aliases = ["uninstall", "un", "rm"])]
    Remove {
        name: String,
        /// Also delete cached Dev OS extension release directories
        #[arg(long)]
        purge: bool,
    },
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
    /// Update packages and extensions from their GitHub latest release
    Update {
        /// Only update these packages or extensions (default: all configurable)
        names: Vec<String>,
    },
    /// Update the dev binary itself from the d-osc/DevOsPM latest release
    SelfUpdate,
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
    if let Commands::Update { names } = &cli.command {
        return update::run(cli.root, cli.devos_data, cli.applications, names);
    }
    if let Commands::SelfUpdate = cli.command {
        // Replacing the binary never touches the package store, so no lock.
        return update::self_update();
    }
    // Download before acquiring the store lock so slow networks do not block readers.
    let downloaded = if let Commands::Install { archive, .. } = &cli.command {
        progress::stage("Preparing installation...");
        download::resolve(archive)?
    } else {
        None
    };
    let write = matches!(
        cli.command,
        Commands::Install { .. } | Commands::Remove { .. } | Commands::Rollback { .. }
    );
    if matches!(cli.command, Commands::Install { .. }) {
        progress::stage("Opening package store (waiting for any active operation)...");
    }
    let mut manager = Manager::open_with(cli.root, cli.devos_data, cli.applications, write)?;
    match cli.command {
        Commands::Install {
            archive,
            name,
            version,
            sha256,
            replace,
            no_desktop,
        } => {
            let source_repository = archive
                .contains("://")
                .then(|| update::repository_slug(&archive))
                .flatten();
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
                    no_desktop,
                    source_repository,
                },
            )?;
            progress::finish();
            match snapshot {
                InstallOutcome::Package(snapshot) => {
                    println!(
                        "Installed {} {}",
                        snapshot.manifest.name, snapshot.manifest.version
                    );
                    println!("Path: {}", manager.snapshot_path(&snapshot).display());
                    for command in snapshot.manifest.bin.keys() {
                        match manager.shim_path(&snapshot.manifest.name, command) {
                            Some(_) => println!("Run: {command}   (or: dev run {command})"),
                            None => println!("Run: dev run {command}"),
                        }
                    }
                }
                InstallOutcome::Extension(entry) => {
                    println!("Installed Dev OS extension {} {}", entry.id, entry.version);
                    println!("Path: {}", manager.extension_path(&entry).display());
                    println!("Restart the Dev OS session to load the extension.");
                }
            }
        }
        Commands::List { json } => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "packages": manager.packages(),
                        "extensions": manager.extensions(),
                    })
                );
            } else if manager.packages().is_empty() && manager.extensions().is_empty() {
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
                for (id, entry) in manager.extensions() {
                    println!(
                        "{id}\t{}\t{} files\t{} bytes\tdev-os extension",
                        entry.version,
                        entry.files.len(),
                        entry.bytes
                    );
                }
            }
        }
        Commands::Info { name } => {
            if let Ok(package) = manager.package(&name) {
                println!("{}", serde_json::to_string_pretty(package)?);
                println!("Path: {}", manager.snapshot_path(&package.active).display());
                if let Some(release) = package.active.manifest.resolved_release()? {
                    println!("Release: {}", serde_json::to_string_pretty(&release)?);
                }
            } else {
                let entry = manager.extension(&name)?;
                println!("{}", serde_json::to_string_pretty(entry)?);
                println!("Path: {}", manager.extension_path(entry).display());
            }
        }
        Commands::Verify { name } => {
            let count = manager.verify(name.as_deref())?;
            println!("Verified {count} package(s): all files intact");
        }
        Commands::Remove { name, purge } => {
            if purge && manager.extensions().contains_key(&name) {
                manager.remove_extension(&name, true)?;
            } else {
                manager.remove(&name)?;
            }
            println!("Removed {name}");
        }
        Commands::Rollback { name } => {
            let version = manager.rollback(&name)?;
            println!("Rolled back {name} to {version}");
        }
        Commands::Run { command, args } => return manager.run(&command, &args),
        Commands::Pack { .. } | Commands::Update { .. } | Commands::SelfUpdate => unreachable!(),
    }
    Ok(0)
}

fn main() {
    let cli = Cli::parse();
    match execute(cli) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            progress::fail();
            eprintln!("dev: {error}");
            std::process::exit(1);
        }
    }
}
