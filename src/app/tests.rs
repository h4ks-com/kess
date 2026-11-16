use std::time::Duration;

use super::*;

fn settings() -> Settings {
    Settings {
        host: Side::White,
        control: Some("5+3".parse().unwrap()),
    }
}

fn game(mode: Mode, style: Style) -> Kess {
    Kess::new(
        Art::load().unwrap(),
        Fonts::load().unwrap(),
        mode,
        settings(),
        style,
        Speaker::open(true),
    )
}

fn type_keys(kess: &mut Kess, presses: &[Key]) {
    for press in presses {
        kess.key(*press);
    }
}

fn step_for(kess: &mut Kess, seconds: f32) {
    for _ in 0..(seconds * 60.0) as u32 {
        kess.step(1.0 / 60.0);
    }
}

fn moves(kess: &Kess) -> Vec<String> {
    kess.table.as_ref().unwrap().snapshot().moves.clone()
}

/// The frame the runner would send, as RGBA bytes.
fn frame(kess: &mut Kess, width: u32, height: u32) -> Vec<u8> {
    let mut light = Light::new(width, height, 1.0);
    kess.render(&mut light);
    let surface = light.to_surface();
    let mut pixels = surface.pixels().to_vec();
    kess.overlay(Frame {
        pixels: &mut pixels,
        width: width as usize,
        height: height as usize,
        aspect: 1.0,
    });
    pixels
}

fn shoot(kess: &mut Kess, width: u32, height: u32, folder: &str, name: &str) {
    let pixels = frame(kess, width, height);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(folder);
    std::fs::create_dir_all(&out).unwrap();
    let rgba = image::RgbaImage::from_raw(width, height, pixels).unwrap();
    let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
    rgb.save(out.join(format!("{name}.jpg"))).unwrap();
}

#[test]
fn arrows_and_hjkl_both_play_e4() {
    let mut kess = game(Mode::Hotseat, Style::Amber);
    type_keys(
        &mut kess,
        &[Key::Enter, Key::Up, Key::Char('k'), Key::Enter],
    );
    assert_eq!(moves(&kess), ["e2e4"]);
    assert_eq!(kess.picked, None);
}

#[test]
fn a_click_on_a_piece_then_a_square_moves_it() {
    let mut kess = game(Mode::Hotseat, Style::Glass);
    frame(&mut kess, 800, 500);
    let stage = kess.stage.as_ref().unwrap();
    let knight = stage.spot([6.5, 0.5], 0.6).unwrap();
    let f3 = stage.spot([5.5, 2.5], 0.0).unwrap();
    kess.pointer(MouseAction::Press(Button::Left), knight);
    kess.pointer(MouseAction::Release(Button::Left), knight);
    assert_eq!(kess.picked, Some(Square::G1));
    kess.pointer(MouseAction::Press(Button::Left), f3);
    kess.pointer(MouseAction::Release(Button::Left), f3);
    assert_eq!(moves(&kess), ["g1f3"]);
}

#[test]
fn a_drag_turns_the_board_instead_of_clicking() {
    let mut kess = game(Mode::Hotseat, Style::Metal);
    frame(&mut kess, 800, 500);
    let home = kess.orbit;
    kess.pointer(MouseAction::Press(Button::Left), P2::new(400.0, 250.0));
    kess.pointer(MouseAction::Drag(Button::Left), P2::new(460.0, 240.0));
    kess.pointer(MouseAction::Release(Button::Left), P2::new(460.0, 240.0));
    assert!(kess.orbit.yaw < home.yaw);
    assert_eq!(kess.picked, None);
    type_keys(&mut kess, &[Key::Char('c')]);
    assert_eq!(kess.orbit, home);
}

#[test]
fn a_click_on_the_flat_board_moves_from_black_too() {
    let mut kess = game(Mode::Hotseat, Style::Amber);
    type_keys(&mut kess, &[Key::Char('t'), Key::Char('f')]);
    play_out(&mut kess, &["e2e4"]);
    frame(&mut kess, 900, 400);
    let stage = kess.stage.as_ref().unwrap();
    let pawn = stage.spot([3.5, 6.5], 0.0).unwrap();
    let d5 = stage.spot([3.5, 4.5], 0.0).unwrap();
    assert!(pawn.y > d5.y, "black's pawns sit at the bottom");
    for spot in [pawn, d5] {
        kess.pointer(MouseAction::Press(Button::Left), spot);
        kess.pointer(MouseAction::Release(Button::Left), spot);
    }
    assert_eq!(moves(&kess), ["e2e4", "d7d5"]);
    assert!(kess.orbit.flat);
}

#[test]
fn dragging_on_past_the_top_flattens_the_board_and_back_down_raises_it() {
    let mut kess = game(Mode::Hotseat, Style::Glass);
    frame(&mut kess, 800, 500);
    let drag = |kess: &mut Kess, to: f32| {
        kess.pointer(MouseAction::Press(Button::Left), P2::new(400.0, 250.0));
        kess.pointer(MouseAction::Drag(Button::Left), P2::new(400.0, to));
        kess.pointer(MouseAction::Release(Button::Left), P2::new(400.0, to));
    };
    drag(&mut kess, 300.0);
    assert!(!kess.orbit.flat);
    drag(&mut kess, 420.0);
    assert!(kess.orbit.flat);
    drag(&mut kess, 150.0);
    assert!(!kess.orbit.flat);
}

#[test]
fn the_bot_answers_without_blocking() {
    let mut kess = game(Mode::Computer { human: Side::White }, Style::Metal);
    type_keys(&mut kess, &[Key::Enter, Key::Up, Key::Up, Key::Enter]);
    step_for(&mut kess, 0.2);
    assert_eq!(moves(&kess).len(), 1);
    for _ in 0..200 {
        step_for(&mut kess, 0.05);
        std::thread::sleep(Duration::from_millis(5));
        if moves(&kess).len() == 2 {
            break;
        }
    }
    assert_eq!(moves(&kess).len(), 2);
    assert!(kess.my_turn());
}

#[test]
fn a_held_r_never_resigns_on_its_own() {
    let mut kess = game(Mode::Hotseat, Style::Amber);
    type_keys(&mut kess, &[Key::Char('r'), Key::Char('r'), Key::Char('r')]);
    assert!(!kess.table.as_ref().unwrap().is_over());
    type_keys(&mut kess, &[Key::Char('y')]);
    assert!(kess.table.as_ref().unwrap().is_over());
}

#[test]
fn a_resting_board_stops_redrawing() {
    let mut kess = game(Mode::Hotseat, Style::Amber);
    frame(&mut kess, 400, 250);
    kess.take_dirty();
    step_for(&mut kess, 0.5);
    assert!(!kess.busy());
    assert!(!kess.take_dirty());
}

#[test]
fn clock_text_switches_to_tenths_under_ten_seconds() {
    assert_eq!(clock_text(185_000), "3:05");
    assert_eq!(clock_text(9_450), "9.4");
    assert_eq!(clock_text(-5), "0.0");
}

/// Play `moves` on the table and let each one land.
fn play_out(kess: &mut Kess, moves: &[&str]) {
    for (ply, uci) in moves.iter().enumerate() {
        let side = if ply % 2 == 0 {
            Side::White
        } else {
            Side::Black
        };
        let change = kess.table.as_mut().unwrap().play(side, uci, 0).unwrap();
        kess.react(change);
        step_for(kess, 0.4);
    }
}

/// `just screenshots` renders these into docs/screenshots.
#[test]
#[ignore = "writes the documentation screenshots"]
fn screenshots() {
    let folder = std::env::var("KESS_SHOTS").unwrap_or_else(|_| "target/shots".into());
    let opening = [
        "e2e4", "e7e5", "g1f3", "b8c6", "f1c4", "g8f6", "f3g5", "d7d5", "e4d5",
    ];
    for (style, white, pick) in [
        (Style::Glass, false, Some(Square::F6)),
        (Style::Metal, true, Some(Square::C4)),
    ] {
        let mut kess = game(Mode::Hotseat, style);
        kess.orbit = kess
            .art
            .lens
            .home(white)
            .swung(if white { -0.15 } else { 0.15 }, 0.0);
        play_out(&mut kess, &opening);
        step_for(&mut kess, 2.0);
        kess.picked = pick;
        kess.cursor = pick.map_or(kess.cursor, |square| {
            (square.file() as usize, square.rank() as usize)
        });
        shoot(&mut kess, 1600, 1000, &folder, style.name());
    }
    let mut kess = game(Mode::Hotseat, Style::Amber);
    kess.orbit.flat = true;
    play_out(&mut kess, &opening);
    step_for(&mut kess, 2.0);
    kess.picked = Some(Square::F6);
    kess.cursor = (5, 5);
    shoot(&mut kess, 1600, 1000, &folder, "flat");
    let mut kess = game(Mode::Hotseat, Style::Glass);
    play_out(&mut kess, &["e2e4", "d7d5"]);
    let change = kess
        .table
        .as_mut()
        .unwrap()
        .play(Side::White, "e4d5", 0)
        .unwrap();
    kess.react(change);
    step_for(&mut kess, 0.42);
    shoot(&mut kess, 1600, 1000, &folder, "capture");
}

/// Prints how long frames take at 1600 by 1000: `cargo test --release frame_times -- --ignored --nocapture`.
#[test]
#[ignore = "measures time"]
fn frame_times() {
    let mut kess = game(Mode::Hotseat, Style::Amber);
    let mut light = Light::new(1600, 1000, 1.0);
    let time = |label: &str, kess: &mut Kess, light: &mut Light, rounds: u32| {
        let started = Instant::now();
        for _ in 0..rounds {
            kess.step(1.0 / 60.0);
            kess.render(light);
            let surface = light.to_surface();
            let mut pixels = surface.pixels().to_vec();
            kess.overlay(Frame {
                pixels: &mut pixels,
                width: 1600,
                height: 1000,
                aspect: 1.0,
            });
        }
        let each = started.elapsed() / rounds;
        let mut out = std::io::stderr();
        let _ = std::io::Write::write_fmt(&mut out, format_args!("{label}: {each:?} a frame\n"));
    };
    time("first frame, board built", &mut kess, &mut light, 1);
    time("resting", &mut kess, &mut light, 20);
    let change = kess
        .table
        .as_mut()
        .unwrap()
        .play(Side::White, "e2e4", 0)
        .unwrap();
    kess.react(change);
    time("piece sliding", &mut kess, &mut light, 15);
    kess.pointer(MouseAction::Press(Button::Left), P2::new(800.0, 500.0));
    for step in 1..=10 {
        kess.pointer(
            MouseAction::Drag(Button::Left),
            P2::new(800.0 + step as f32 * 12.0, 500.0),
        );
        time("board turning", &mut kess, &mut light, 1);
    }
}

fn online(code: &RoomCode, seat: Seat) -> Kess {
    let mode = Mode::Online {
        link: Rc::new(Link::open(code, seat)),
        seat,
        code: code.clone(),
    };
    game(mode, Style::Glass)
}

fn wait(host: &mut Kess, guest: &mut Kess, done: impl Fn(&Kess, &Kess) -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed().as_secs() < 20 {
        host.step(1.0 / 60.0);
        guest.step(1.0 / 60.0);
        if done(host, guest) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    false
}

#[test]
fn a_host_copies_its_code_once_and_again_on_y() {
    let code = RoomCode::random().unwrap();
    let mut host = online(&code, Seat::Host);
    assert_eq!(host.take_clipboard(), Some(code.to_string()));
    assert_eq!(host.take_clipboard(), None);
    type_keys(&mut host, &[Key::Char('y')]);
    assert_eq!(host.take_clipboard(), Some(code.to_string()));
    let mut guest = online(&code, Seat::Guest);
    assert_eq!(guest.take_clipboard(), None);
}

#[test]
fn a_departed_opponent_ends_the_game_for_good() {
    let code = RoomCode::random().unwrap();
    let mut host = online(&code, Seat::Host);
    host.receive(Message::Join { protocol: PROTOCOL });
    assert!(host.my_turn());
    host.receive(Message::Leave);
    assert!(!host.my_turn());
    type_keys(&mut host, &[Key::Char('n'), Key::Char('d'), Key::Char('r')]);
    assert_eq!(host.resign_armed, 0.0);
    assert!(
        host.table
            .as_ref()
            .is_some_and(|table| table.verdict().is_none())
    );
}

#[test]
#[ignore = "needs the internet"]
fn two_players_meet_and_trade_moves_through_the_brokers() {
    let code = RoomCode::random().unwrap();
    let mut host = online(&code, Seat::Host);
    let mut guest = online(&code, Seat::Guest);
    assert!(wait(&mut host, &mut guest, |host, guest| host.joined()
        && guest.joined()));
    assert_eq!(guest.local(), [Side::Black]);
    type_keys(&mut host, &[Key::Enter, Key::Up, Key::Up, Key::Enter]);
    let moved =
        |_: &Kess, guest: &Kess| guest.table.as_ref().is_some_and(|t| t.record().len() == 1);
    assert!(wait(&mut host, &mut guest, moved));
    type_keys(&mut guest, &[Key::Enter, Key::Up, Key::Up, Key::Enter]);
    let answered =
        |host: &Kess, _: &Kess| host.table.as_ref().is_some_and(|t| t.record().len() == 2);
    assert!(wait(&mut host, &mut guest, answered));
    assert_eq!(moves(&host), ["e2e4", "e7e5"]);
    if let Mode::Online { link, .. } = &guest.mode {
        link.close();
    }
    assert!(wait(&mut host, &mut guest, |host, _| host.opponent
        == Opponent::Gone));
    if let Mode::Online { link, .. } = &host.mode {
        link.close();
    }
}
