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

    /// External subcommand — forwarded to `windy-<name>` if present in PATH.
    ///
    /// Lets ecosystem tools graft onto the `windy` CLI without windy-lang
    /// depending on them. For example, `windy mine path/foo.wnd` looks up
    /// `windy-mine` in PATH (shipped by `windy-coin`'s `cli/` crate) and
    /// execs it with the remaining arguments.
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
        Command::External(args) => run_plugin(&args),
    }
}

/// Git-style plugin dispatch: `windy <subcmd> [args...]` execs
/// `windy-<subcmd>` from PATH, forwarding `[args...]` verbatim. The
/// exit code of the plugin becomes our exit code. If the plugin
/// isn't installed, we print a hint and exit 127 (POSIX
/// "command not found").
fn run_plugin(args: &[String]) -> ProcessExit {
    let Some(subcmd) = args.first() else {
        eprintln!("windy: no subcommand given");
        return ProcessExit::from(2);
    };
    let plugin = format!("windy-{subcmd}");
    let rest = &args[1..];

    match ProcessCommand::new(&plugin).args(rest).status() {
        Ok(status) => {
            let code = status.code().unwrap_or(1);
            ProcessExit::from(code.clamp(0, 255) as u8)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            eprintln!("windy: '{subcmd}' is not a windy command.");
            eprintln!("       (also tried '{plugin}' — not found in PATH)");
            eprintln!();
            if subcmd == "mine" {
                eprintln!("To mine WNDY, install windy-mine from windy-coin:");
                eprintln!(
                    "  cargo install --git https://github.com/sisobus/windy-coin windy-mine"
                );
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
