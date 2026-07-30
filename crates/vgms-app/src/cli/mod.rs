//! `drotrim`'s command line and its subcommands. (DRO v2 -> v1 conversion lives
//! in the GUI: Edit > Convert to DRO v1.)
//!
//! One executable does everything. With no subcommand (and at most a file to
//! open) `drotrim` starts the GUI, so the parser's [`Cli::command`] is optional
//! and the bare `drotrim song.dro` of old still works. `drotrim help` lists the
//! rest, courtesy of clap.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

pub mod optimize;
pub mod play;
pub mod render;
pub mod retrowave;
pub mod split;

#[cfg(windows)]
mod console;
#[cfg(windows)]
pub use console::attach_parent_console;

/// Edit, play, render, split and optimise DRO and VGM songs.
#[derive(Debug, Parser)]
#[command(
    // Without this, clap names the command after the *package* (`vgms-app`),
    // which is not what the user typed.
    name = "drotrim",
    version,
    about = "Edit, play, render, split and optimise DRO and VGM songs.",
    after_help = "Run with no arguments (or with just a file) to open the GUI.",
    // `drotrim song.dro render` is a mistake, not a request to render; reject
    // it rather than half-parse it.
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// A .dro, .vgm or .vgz file to open in the GUI at startup.
    pub file: Option<PathBuf>,
}

/// What to do instead of opening the GUI.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Play a song through the speakers.
    Play(play::Args),
    /// Render a song to a WAV file.
    Render(render::Args),
    /// Split a song into one file per channel used.
    Split(split::Args),
    /// Strip redundant OPL writes from a VGM (the `vgm_cmp` optimisation).
    Optimize(optimize::Args),
    /// List RetroWave OPL3 hardware and play a test chord on it.
    #[command(name = "retrowave-probe")]
    RetrowaveProbe(retrowave::Args),
}

/// Runs a subcommand.
///
/// # Errors
/// Whatever the subcommand reports: an unreadable or unparseable input, a failed
/// write, or (for `play`) no usable audio device.
pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Play(args) => play::run(&args),
        Command::Render(args) => render::run(&args),
        Command::Split(args) => split::run(&args),
        Command::Optimize(args) => optimize::run(&args),
        Command::RetrowaveProbe(args) => retrowave::run(&args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_parser_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_arguments_means_the_gui() {
        let cli = Cli::try_parse_from(["drotrim"]).unwrap();
        assert!(cli.command.is_none());
        assert!(cli.file.is_none());
    }

    #[test]
    fn a_lone_path_is_the_gui_file() {
        let cli = Cli::try_parse_from(["drotrim", "song.dro"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.file, Some(PathBuf::from("song.dro")));
    }

    #[test]
    fn each_subcommand_parses() {
        assert!(matches!(
            Cli::try_parse_from(["drotrim", "play", "a.dro"])
                .unwrap()
                .command,
            Some(Command::Play(_))
        ));
        assert!(matches!(
            Cli::try_parse_from(["drotrim", "render", "a.dro"])
                .unwrap()
                .command,
            Some(Command::Render(_))
        ));
        assert!(matches!(
            Cli::try_parse_from(["drotrim", "split", "a.dro"])
                .unwrap()
                .command,
            Some(Command::Split(_))
        ));
        assert!(matches!(
            Cli::try_parse_from(["drotrim", "optimize", "a.vgm"])
                .unwrap()
                .command,
            Some(Command::Optimize(_))
        ));
        // The optional output path parses too.
        let Some(Command::Optimize(args)) =
            Cli::try_parse_from(["drotrim", "optimize", "a.vgm", "b.vgz"])
                .unwrap()
                .command
        else {
            panic!("expected an optimize command");
        };
        assert_eq!(args.output, Some(PathBuf::from("b.vgz")));
        assert!(matches!(
            Cli::try_parse_from(["drotrim", "retrowave-probe"])
                .unwrap()
                .command,
            Some(Command::RetrowaveProbe(_))
        ));
    }

    #[test]
    fn the_probe_takes_a_port_and_a_list_only_flag() {
        let Some(Command::RetrowaveProbe(args)) =
            Cli::try_parse_from(["drotrim", "retrowave-probe", "--port", "COM3", "--list"])
                .unwrap()
                .command
        else {
            panic!("expected a retrowave-probe command");
        };
        assert_eq!(args.port.as_deref(), Some("COM3"));
        assert!(args.list_only);
    }

    /// The old `--dro` flag, kept working for anyone with it in a script.
    #[test]
    fn the_old_dro_flag_still_selects_song_output() {
        for argv in [
            ["drotrim", "split", "-d", "a.dro"],
            ["drotrim", "split", "--dro", "a.dro"],
        ] {
            let Some(Command::Split(args)) = Cli::try_parse_from(argv).unwrap().command else {
                panic!("expected a split command from {argv:?}")
            };
            assert!(args.song);
        }
    }

    #[test]
    fn a_file_and_a_subcommand_together_are_rejected() {
        assert!(Cli::try_parse_from(["drotrim", "a.dro", "render"]).is_err());
    }
}
