//! Windy CLI — `windy run` / `windy debug` / `windy version`.
//!
//! The v0.1 `compile` subcommand (Python output-baking stopgap) is
//! retired — per-program AOT becomes obsolete once the interpreter
//! itself ships as WebAssembly in v0.3 (SPEC §10).

use clap::{Parser, Subcommand};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, ExitCode as ProcessExit};
use windy::{debug_source, run_source, RunOptions, VERSION};

#[derive(Parser)]
#[command(
    name = "windy",
    version = VERSION,
    about = "Windy — 2D esolang where code flows like wind"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a Windy program on the bytecode VM.
    Run {
        /// Path to the .wnd source file.
        file: PathBuf,
        /// Seed for the ~ (turbulence) RNG.
        #[arg(long)]
        seed: Option<u64>,
        /// Halt after N executed steps (exit 124 if exceeded).
        #[arg(long = "max-steps")]
        max_steps: Option<u64>,
    },
    /// Step through a Windy program interactively.
    Debug {
        /// Path to the .wnd source file.
        file: PathBuf,
    },
    /// Print the Windy version.
    Version,

    /// Mine WNDY tokens via Risc Zero ZK proofs of a Windy program.
    ///
    /// Forwards every argument verbatim to the `windy-mine` binary
    /// (shipped by `windy-coin`). Install with:
    ///
    ///     cargo install --git https://github.com/sisobus/windy-coin windy-mine
    ///
    /// Then either form works:
    ///
    ///     windy mine programs/foo.wnd              # full Risc Zero proof + mainnet mint
    ///     windy mine programs/foo.wnd --dry-run    # score-only, no proof, no tx
    ///
    /// `--help` is forwarded too, so `windy mine --help` shows `windy-mine`'s
    /// own option reference once installed.
    ///
    /// See windy-coin's `docs/MINING-GUIDE.md` for the full guide.
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Mine {
        /// All arguments forwarded to `windy-mine`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// External subcommand — forwarded to `windy-<name>` if present in PATH.
    ///
    /// Lets ecosystem tools graft onto the `windy` CLI without windy-lang
    /// depending on them. `mine` is the first such plugin (windy-coin's
    /// `windy-mine`); any future `windy-<name>` binary in PATH becomes
    /// `windy <name>` automatically.
    #[command(external_subcommand)]
    External(Vec<String>),
}

fn main() -> ProcessExit {
    let cli = Cli::parse();
    match cli.cmd {
        Command::Version => {
            println!("Windy {}", VERSION);
            ProcessExit::from(0)
        }
        Command::Run { file, seed, max_steps } => {
            let source = match fs::read_to_string(&file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("windy: cannot read {}: {}", file.display(), e);
                    return ProcessExit::from(2);
                }
            };
            let mut stdin = io::stdin().lock();
            let mut stdout = io::stdout().lock();
            let mut stderr = io::stderr().lock();
            let code = run_source(
                &source,
                RunOptions {
                    seed,
                    max_steps,
                    stdin: &mut stdin,
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
            );
            ProcessExit::from(code.code() as u8)
        }
        Command::Debug { file } => {
            let source = match fs::read_to_string(&file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("windy: cannot read {}: {}", file.display(), e);
                    return ProcessExit::from(2);
                }
            };
            let mut stdin = io::stdin().lock();
            let code = debug_source(&source, &mut stdin);
            ProcessExit::from(code.clamp(0, 255) as u8)
        }
        Command::Mine { args } => run_plugin("mine", &args),
        Command::External(args) => {
            let Some((subcmd, rest)) = args.split_first() else {
                eprintln!("windy: no subcommand given");
                return ProcessExit::from(2);
            };
            run_plugin(subcmd, rest)
        }
    }
}

/// Git-style plugin dispatch: forward `windy <subcmd> [args...]` to
/// `windy-<subcmd> [args...]` in PATH. Exit code of the plugin becomes
/// our exit code. If the plugin isn't installed, print a hint and exit
/// 127 (POSIX "command not found"). `mine` gets a tailored hint pointing
/// at windy-coin's install command since it's the first ecosystem plugin
/// AND has an explicit subcommand entry — the unknown-plugin branch is
/// reachable for `mine` if windy-mine isn't installed yet.
fn run_plugin(subcmd: &str, args: &[String]) -> ProcessExit {
    let plugin = format!("windy-{subcmd}");

    match ProcessCommand::new(&plugin).args(args).status() {
        Ok(status) => {
            let code = status.code().unwrap_or(1);
            ProcessExit::from(code.clamp(0, 255) as u8)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            eprintln!("windy: '{subcmd}' requires the '{plugin}' plugin, but it isn't in PATH.");
            eprintln!();
            if subcmd == "mine" {
                eprintln!("To mine WNDY, install windy-mine from crates.io:");
                eprintln!("  cargo install windy-mine");
            } else {
                eprintln!(
                    "windy supports git-style plugins: any executable named \
                     `windy-<name>` in PATH becomes a `windy <name>` subcommand."
                );
            }
            ProcessExit::from(127)
        }
        Err(e) => {
            eprintln!("windy: cannot exec '{plugin}': {e}");
            ProcessExit::from(127)
        }
    }
}
