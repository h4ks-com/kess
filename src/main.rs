use std::io::{self, Write};
use std::process::ExitCode;
use std::rc::Rc;

use kess::app::{Kess, Mode};
use kess::art::{Art, Style};
use kess::menu::{Choice, Start};
use kess::net::{Link, Role};
use kess::sound::Speaker;
use kess::table::{Settings, Side, TimeControl};
use kess::text::Fonts;
use kess::wire::RoomCode;

const USAGE: &str = "\
kess, chess in the terminal

usage:
  kess                         open the start menu
  kess local                   play both sides on this keyboard
  kess bot [white|black]       play against the built-in bot
  kess host [white|black]      start an online game and get a code to share
  kess join <code>             join a game with the host's code

options:
  --clock <min+sec>            time control, like 5+3 (default: none)
  --set <amber|glass|metal>    the piece set to start with
  --mute                       start without sound
";

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Clock(#[from] kess::table::TimeControlError),
    #[error(transparent)]
    Code(#[from] kess::wire::WireError),
    #[error(transparent)]
    Art(#[from] kess::art::ArtError),
    #[error(transparent)]
    Font(#[from] kess::text::FontError),
    #[error("the terminal failed: {0}")]
    Terminal(#[from] katui::rt::RtError),
}

struct Options {
    command: Vec<String>,
    control: Option<TimeControl>,
    style: Style,
    muted: bool,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Options, CliError> {
    let mut options = Options {
        command: Vec::new(),
        control: None,
        style: Style::Amber,
        muted: false,
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--clock" => options.control = Some(value(&mut args, "--clock")?.parse()?),
            "--set" => {
                let name = value(&mut args, "--set")?;
                options.style = Style::ALL
                    .into_iter()
                    .find(|style| style.name() == name)
                    .ok_or_else(|| CliError::Usage(format!("no set called {name}")))?;
            }
            "--mute" => options.muted = true,
            "-h" | "--help" => return Err(CliError::Usage(String::new())),
            _ => options.command.push(arg),
        }
    }
    Ok(options)
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, CliError> {
    args.next()
        .ok_or_else(|| CliError::Usage(format!("{flag} needs a value")))
}

fn side(choice: Option<&str>) -> Result<Option<Side>, CliError> {
    match choice {
        Some("white") => Ok(Some(Side::White)),
        Some("black") => Ok(Some(Side::Black)),
        Some(other) => Err(CliError::Usage(format!("pick white or black, not {other}"))),
        None => Ok(None),
    }
}

/// `side`, or a coin toss when the player left it open.
fn settle(side: Option<Side>) -> Result<Side, CliError> {
    if let Some(side) = side {
        return Ok(side);
    }
    let mut coin = [0_u8; 1];
    getrandom::fill(&mut coin).map_err(|_| CliError::Usage("no random source".into()))?;
    Ok(if coin[0] % 2 == 0 {
        Side::White
    } else {
        Side::Black
    })
}

fn online(code: RoomCode, seat: Role) -> (Mode, Option<Rc<Link>>) {
    let link = Rc::new(Link::open(&code, seat));
    let mode = Mode::Online {
        link: Rc::clone(&link),
        seat,
        code,
    };
    (mode, Some(link))
}

/// What the command line asks for, or the start menu's answer when it asks for nothing.
fn choose(options: &Options) -> Result<Option<Choice>, CliError> {
    let words: Vec<&str> = options.command.iter().map(String::as_str).collect();
    let start = match words.as_slice() {
        [] => return Ok(kess::menu::choose(options.style, options.muted)?),
        ["local"] => Start::Hotseat,
        ["host", rest @ ..] if rest.len() <= 1 => Start::Host(side(rest.first().copied())?),
        ["join", code] => Start::Join(code.parse()?),
        ["bot", rest @ ..] if rest.len() <= 1 => Start::Bot(side(rest.first().copied())?),
        _ => return Err(CliError::Usage(String::new())),
    };
    Ok(Some(Choice {
        start,
        control: options.control,
        style: options.style,
        muted: options.muted,
    }))
}

fn run(options: &Options) -> Result<(), CliError> {
    let Some(choice) = choose(options)? else {
        return Ok(());
    };
    let mut settings = Settings {
        host: Side::White,
        control: choice.control,
    };
    let (mode, link) = match choice.start {
        Start::Hotseat => (Mode::Hotseat, None),
        Start::Host(side) => {
            settings.host = settle(side)?;
            online(RoomCode::random()?, Role::Host)
        }
        Start::Join(code) => online(code, Role::Guest),
        Start::Bot(side) => (
            Mode::Computer {
                human: settle(side)?,
            },
            None,
        ),
    };
    let speaker = Speaker::open(choice.muted);
    let game = Kess::new(
        Art::load()?,
        Fonts::load()?,
        mode,
        settings,
        choice.style,
        speaker,
    );
    let played = kess::runner::run(game);
    if let Some(link) = link {
        link.close();
    }
    Ok(played?)
}

fn main() -> ExitCode {
    let result = parse(std::env::args().skip(1)).and_then(|options| run(&options));
    let mut stderr = io::stderr();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(problem)) => {
            if !problem.is_empty() {
                let _ = writeln!(stderr, "{problem}\n");
            }
            let _ = write!(stderr, "{USAGE}");
            ExitCode::from(2)
        }
        Err(error) => {
            let _ = writeln!(stderr, "kess: {error}");
            ExitCode::FAILURE
        }
    }
}
