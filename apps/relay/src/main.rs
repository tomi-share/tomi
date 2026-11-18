//! `tomi-relay`: without argument, runs the relay; `tomi-relay admin ...` administers it
//! from the machine (`api-v1.md` §8.7).

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tomi_relay::admin::{self, Cmd, WebAction};
use tomi_relay::app::{self, Options, PushChoice};
use tomi_relay::config::Config;
use tomi_relay::logging;

#[derive(Parser)]
#[command(
    name = "tomi-relay",
    version,
    about = "Tomi relay: a blind store-and-forward server"
)]
struct Cli {
    /// Configuration file (TOML). Optional: `TOMI_RELAY_*` variables override its keys.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Runs the relay (the default).
    Serve,
    /// Local administration; never reachable from the network.
    Admin {
        #[command(subcommand)]
        cmd: AdminCommand,
    },
}

#[derive(Subcommand)]
enum AdminCommand {
    /// State of the relay (spaces, total quota, disk, push health), or of one space.
    Status {
        #[arg(long)]
        json: bool,
        /// Show this space instead (its id or the first 8 characters).
        #[arg(long, value_name = "ID")]
        space: Option<String>,
    },
    /// The member devices of a space.
    Devices {
        #[arg(long)]
        json: bool,
        /// The space (its id or the first 8 characters); optional if the relay has only one.
        #[arg(long, value_name = "ID")]
        space: Option<String>,
    },
    /// The spaces of the relay: id, date, members, usage, quota.
    Spaces {
        #[arg(long)]
        json: bool,
    },
    /// Sets the quota of a space: a number of bytes, `none` (no limit) or `default`
    /// (the default quota of the relay: the one set from the web panel, else `max_space_bytes`, else no limit).
    Quota {
        /// The space (its id or the first 8 characters); optional if the relay has only one.
        #[arg(long, value_name = "ID")]
        space: Option<String>,
        /// Bytes, `none` or `default`.
        value: String,
    },
    /// Lists the pending invitations (permissions to create a space).
    Invite {
        /// Creates an invitation and prints its installation token (shown only now).
        #[arg(long = "new", conflicts_with = "rotate")]
        new: bool,
        /// Replaces every pending invitation by a new one and prints its token.
        #[arg(long)]
        rotate: bool,
    },
    /// Revokes a device (id or prefix of at least 8 characters).
    Revoke {
        target: String,
        /// The space (its id or the first 8 characters); optional if the relay has only one.
        #[arg(long, value_name = "ID")]
        space: Option<String>,
    },
    /// Deletes a whole space and nothing else. Prints a new installation token if it was the
    /// last space.
    Reset {
        /// The space (its id or the first 8 characters); optional if the relay has only one.
        #[arg(long, value_name = "ID")]
        space: Option<String>,
        /// The first 8 characters of the space id (otherwise asked on the terminal).
        #[arg(long)]
        confirm: Option<String>,
    },
    /// The web administration panel (needs `admin_web = true`): shows its URL; with
    /// `--rotate-path` makes a new secret path; `enroll` and `reset` manage the passkey.
    #[command(args_conflicts_with_subcommands = true)]
    Web {
        /// Generates a new secret path: the old URL and every session stop working.
        #[arg(long)]
        rotate_path: bool,
        #[command(subcommand)]
        action: Option<WebSub>,
    },
}

#[derive(Subcommand)]
enum WebSub {
    /// Makes a one-time enrolment code (shown once, valid 15 minutes, 5 attempts) to register the
    /// first passkey.
    Enroll,
    /// Removes the passkey and closes every session; the enrolment then needs a new code.
    Reset {
        /// The first 8 characters of the panel path (otherwise asked on the terminal).
        #[arg(long)]
        confirm: Option<String>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = match cli.command {
        None | Some(Command::Serve) => serve(cli.config).await,
        Some(Command::Admin { cmd }) => administer(cli.config, cmd).await,
    };
    ExitCode::from(code)
}

fn fail(msg: impl std::fmt::Display) -> u8 {
    eprintln!("tomi-relay: {msg}");
    1
}

/// A relay (or an `admin` command) that could not open its data: prints why and returns its exit
/// code, [`tomi_relay::spaces::EXIT_DATA_REFUSED`] if the cause is the data (`api-v1.md` §10.1).
fn fail_start(e: &anyhow::Error, hint: &str) -> u8 {
    let code = tomi_relay::spaces::start_exit_code(e);
    fail(format!("{e:#}{hint}"));
    code
}

async fn serve(config: Option<PathBuf>) -> u8 {
    let cfg = match Config::load(config.as_deref()) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    logging::init(cfg.log_format);
    let addr = match cfg.listen_addr() {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    let state = match app::open(cfg, Options::default()).await {
        Ok(s) => s,
        Err(e) => return fail_start(&e, ""),
    };
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => return fail(format!("cannot listen on {addr}: {e}")),
    };
    // A fresh install has no space and no invitation: one is created and its token logged.
    // A write that fails in `routing.db` is a refusal because of the data (`78`).
    if let Err(e) = app::announce_invitation(&state) {
        fail(format!("cannot create the first invitation: {e:#}"));
        return tomi_relay::spaces::EXIT_DATA_REFUSED;
    }
    #[cfg(unix)]
    if let Err(e) = admin::socket::spawn(state.clone()) {
        tracing::warn!("the administration socket is not available: {e}");
    }
    tracing::info!(listen = %addr, version = env!("CARGO_PKG_VERSION"), "tomi-relay started");
    let result = app::serve(state.clone(), listener, shutdown_signal()).await;
    #[cfg(unix)]
    let _ = std::fs::remove_file(admin::socket::path(&state.cfg.data_dir));
    match result {
        Ok(()) => {
            tracing::info!("tomi-relay stopped");
            0
        }
        Err(e) => fail(e),
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn administer(config: Option<PathBuf>, cmd: AdminCommand) -> u8 {
    let cfg = match Config::load(config.as_deref()) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let cmd = match cmd {
        AdminCommand::Status { json, space } => Cmd::Status { json, space },
        AdminCommand::Devices { json, space } => Cmd::Devices { json, space },
        AdminCommand::Spaces { json } => Cmd::Spaces { json },
        AdminCommand::Quota { space, value } => Cmd::Quota { space, value },
        AdminCommand::Invite { new, rotate } => Cmd::Invite { rotate, new },
        AdminCommand::Revoke { target, space } => Cmd::Revoke { target, space },
        AdminCommand::Reset { space, confirm } => Cmd::Reset {
            confirm: confirm.or_else(ask_confirmation),
            space,
        },
        AdminCommand::Web {
            rotate_path,
            action,
        } => Cmd::Web {
            action: match (rotate_path, action) {
                (_, Some(WebSub::Enroll)) => WebAction::Enroll,
                (_, Some(WebSub::Reset { confirm })) => WebAction::Reset {
                    confirm: confirm.or_else(ask_web_confirmation),
                },
                (true, None) => WebAction::RotatePath,
                (false, None) => WebAction::Show,
            },
        },
    };

    // A running relay answers on its socket (Unix).
    #[cfg(unix)]
    if let Some(out) = admin::socket::send(&cfg.data_dir, &cmd).await {
        return print(&out);
    }

    // Otherwise the command works on the databases. What changes something needs the relay
    // stopped (the lock proves it, and only then may the data directory be migrated); reading
    // is always allowed (SQLite WAL) and writes nothing, not even `routing.db`.
    let exclusive = admin::needs_exclusive_access(&cmd);
    let opts = Options {
        push: PushChoice::Disabled,
        background: false,
        hold_lock: exclusive,
        read_only: !exclusive,
        cleanup: exclusive,
        ..Options::default()
    };
    let state = match app::open(cfg, opts).await {
        Ok(s) => s,
        Err(e) => {
            return fail_start(
                &e,
                if exclusive {
                    "\nthis command changes the relay: stop the relay first (there is no administration socket on this platform)"
                } else {
                    ""
                },
            )
        }
    };
    print(&admin::run(&state, cmd).await)
}

fn ask_confirmation() -> Option<String> {
    eprint!("This deletes a whole space. Retype the first 8 characters of its id (see `admin spaces`): ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok()?;
    let t = line.trim().to_owned();
    (!t.is_empty()).then_some(t)
}

fn ask_web_confirmation() -> Option<String> {
    eprint!("This removes the passkey of the web panel and closes its sessions. Retype the first 8 characters of the panel path (see `admin web`): ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok()?;
    let t = line.trim().to_owned();
    (!t.is_empty()).then_some(t)
}

/// Prints the output of a command and returns its exit code.
fn print(out: &admin::Output) -> u8 {
    if !out.stdout.is_empty() {
        print!("{}", out.stdout);
    }
    if !out.stderr.is_empty() {
        eprintln!("{}", out.stderr);
    }
    u8::try_from(out.exit).unwrap_or(1)
}
