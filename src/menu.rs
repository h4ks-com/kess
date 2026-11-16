use std::cell::RefCell;
use std::rc::Rc;

use katui::app::{Control, UiApp, run};
use katui::draw::Paint;
use katui::proto::input::{InputEvent, Key};
use katui::rt::RtError;
use katui::scene::surface::Rgba;
use katui::ui::{Align, CellPixels, Element, Length};
use katui::widgets::{
    Edit, TextField, Theme, hint, key_hints, label, panel, primary_button, switch, tabs,
    text_field, title,
};

use crate::art::Style;
use crate::table::{Side, TimeControl};
use crate::wire::RoomCode;

const MODES: [&str; 4] = ["Play the bot", "Host a game", "Join a game", "Two players"];
const SIDES: [&str; 3] = ["White", "Black", "Coin toss"];
const CLOCKS: [&str; 6] = ["No clock", "1+0", "3+2", "5+3", "10+5", "15+10"];
const SETS: [&str; 3] = ["Amber", "Glass", "Metal"];

/// What the player picked on the start menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// Play the bot; `None` tosses a coin for the side.
    Bot(Option<Side>),
    Host(Option<Side>),
    Join(RoomCode),
    Hotseat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub start: Start,
    pub control: Option<TimeControl>,
    pub style: Style,
    pub muted: bool,
}

/// One row of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Mode,
    Side,
    Clock,
    Code,
    Set,
    Sound,
    Begin,
}

#[derive(Debug, Clone)]
pub enum Message {
    Pick(Field, usize),
    Focus(Field),
    ToggleSound,
    Begin,
}

/// The start menu: what to play, and how.
pub struct Menu {
    mode: usize,
    side: usize,
    clock: usize,
    set: usize,
    sound: bool,
    code: TextField,
    focus: Field,
    problem: Option<String>,
    chosen: Rc<RefCell<Option<Choice>>>,
}

/// Show the start menu until the player starts a game or quits.
pub fn choose(style: Style, muted: bool) -> Result<Option<Choice>, RtError> {
    let chosen = Rc::new(RefCell::new(None));
    let menu = Menu {
        mode: 0,
        side: 0,
        clock: 0,
        set: Style::ALL
            .iter()
            .position(|known| *known == style)
            .unwrap_or(0),
        sound: !muted,
        code: TextField::default(),
        focus: Field::Mode,
        problem: None,
        chosen: Rc::clone(&chosen),
    };
    run(menu)?;
    Ok(chosen.take())
}

impl Menu {
    /// The fields this mode shows, top to bottom.
    fn fields(&self) -> Vec<Field> {
        let middle: &[Field] = match self.mode {
            0 | 1 => &[Field::Side, Field::Clock],
            2 => &[Field::Code],
            _ => &[Field::Clock],
        };
        [Field::Mode]
            .into_iter()
            .chain(middle.iter().copied())
            .chain([Field::Set, Field::Sound, Field::Begin])
            .collect()
    }

    fn step_focus(&mut self, forward: bool) {
        let fields = self.fields();
        let here = fields
            .iter()
            .position(|field| *field == self.focus)
            .unwrap_or(0);
        let next = turn(here, forward, fields.len());
        self.focus = fields.get(next).copied().unwrap_or(Field::Mode);
    }

    /// Change the focused field's value one step forward or back.
    fn nudge(&mut self, forward: bool) {
        let (value, count) = match self.focus {
            Field::Mode => (&mut self.mode, MODES.len()),
            Field::Side => (&mut self.side, SIDES.len()),
            Field::Clock => (&mut self.clock, CLOCKS.len()),
            Field::Set => (&mut self.set, SETS.len()),
            Field::Sound => {
                self.sound = !self.sound;
                return;
            }
            Field::Code | Field::Begin => return,
        };
        *value = turn(*value, forward, count);
        self.problem = None;
    }

    fn begin(&mut self) -> Control {
        let side = match self.side {
            0 => Some(Side::White),
            1 => Some(Side::Black),
            _ => None,
        };
        let start = match self.mode {
            0 => Start::Bot(side),
            1 => Start::Host(side),
            2 => match self.code.text().parse::<RoomCode>() {
                Ok(code) => Start::Join(code),
                Err(error) => {
                    self.problem = Some(error.to_string());
                    self.focus = Field::Code;
                    return Control::Redraw;
                }
            },
            _ => Start::Hotseat,
        };
        let control = CLOCKS.get(self.clock).and_then(|text| text.parse().ok());
        *self.chosen.borrow_mut() = Some(Choice {
            start,
            control,
            style: Style::ALL.get(self.set).copied().unwrap_or(Style::Amber),
            muted: !self.sound,
        });
        Control::Quit
    }

    /// A labeled row; the focused one gets an accent border.
    fn row(&self, field: Field, name: &str, control: Element<Message>) -> Element<Message> {
        let theme = Theme::current();
        let focused = self.focus == field;
        let border = if focused {
            Paint::Solid(theme.accent)
        } else {
            theme.border.clone()
        };
        let caption = label(name).color(if focused { theme.accent } else { theme.muted });
        Element::row([caption.width(theme.columns(12.0)), control])
            .gap(theme.cell_width() * 2.0)
            .align(Align::Center)
            .padding_xy(theme.cell_width() * 1.5, theme.cell_height() * 0.4)
            .radius(theme.radius())
            .border(if focused { 2.0 } else { 1.0 }, border)
            .width(Length::Fill)
            .on_click(Message::Focus(field))
    }

    fn choices(&self, field: Field, names: &[&str], picked: usize, name: &str) -> Element<Message> {
        self.row(
            field,
            name,
            tabs(names, picked, move |index| Message::Pick(field, index)),
        )
    }

    fn field_view(&self, field: Field) -> Option<Element<Message>> {
        Some(match field {
            Field::Side => self.choices(field, &SIDES, self.side, "Your side"),
            Field::Clock => self.choices(field, &CLOCKS, self.clock, "Clock"),
            Field::Set => self.choices(field, &SETS, self.set, "Piece set"),
            Field::Code => {
                let input = text_field(
                    &self.code,
                    "the host's code, like 7FQ2-K9XM-3TBW",
                    self.focus == field,
                );
                self.row(field, "Room code", input)
            }
            Field::Sound => self.row(
                field,
                "Sound",
                switch(
                    if self.sound { "on" } else { "off" },
                    self.sound,
                    Message::ToggleSound,
                ),
            ),
            Field::Mode | Field::Begin => return None,
        })
    }

    fn blurb(&self) -> &'static str {
        match self.mode {
            0 => "Play against the built-in bot on this keyboard.",
            1 => "Get a code to send to a friend, then wait for them to join.",
            2 => "Type the code your friend sent you.",
            _ => "Both sides take turns on this keyboard.",
        }
    }
}

/// What a key asks the menu to do.
enum Intent {
    Quit,
    Begin,
    /// Move to the next row, or the previous one.
    Row(bool),
    /// Change the row's value forward, or back.
    Value(bool),
}

/// `key` read as arrows, vim keys or Tab; letters stay text while `typing` a code.
fn intent(key: Key, shift: bool, typing: bool) -> Option<Intent> {
    let key = if typing { key } else { unvim(key) };
    Some(match key {
        Key::Escape => Intent::Quit,
        Key::Enter => Intent::Begin,
        Key::Tab => Intent::Row(!shift),
        Key::Down => Intent::Row(true),
        Key::Up => Intent::Row(false),
        Key::Left => Intent::Value(false),
        Key::Right | Key::Char(' ') => Intent::Value(true),
        _ => return None,
    })
}

/// hjkl as the arrows they stand for, and q as Esc.
fn unvim(key: Key) -> Key {
    match key {
        Key::Char('h') => Key::Left,
        Key::Char('j') => Key::Down,
        Key::Char('k') => Key::Up,
        Key::Char('l') => Key::Right,
        Key::Char('q') => Key::Escape,
        other => other,
    }
}

/// The next or previous of `count` places after `index`, wrapping around.
fn turn(index: usize, forward: bool, count: usize) -> usize {
    let count = count.max(1);
    if forward {
        (index + 1) % count
    } else {
        (index + count - 1) % count
    }
}

impl UiApp for Menu {
    type Message = Message;

    fn view(&self) -> Element<Message> {
        let theme = Theme::current();
        let mode_tabs = tabs(&MODES, self.mode, |index| Message::Pick(Field::Mode, index));
        let mut rows = vec![
            title("kess"),
            hint("Chess in your terminal"),
            self.row(Field::Mode, "Mode", mode_tabs),
            hint(self.blurb()),
        ];
        rows.extend(
            self.fields()
                .into_iter()
                .filter_map(|field| self.field_view(field)),
        );
        if let Some(problem) = &self.problem {
            rows.push(label(problem.as_str()).color(theme.danger));
        }
        let start = primary_button("Start", Message::Begin);
        let start = if self.focus == Field::Begin {
            start.border(2.0, Paint::Solid(theme.text))
        } else {
            start
        };
        rows.push(
            Element::row([start])
                .justify(Align::Center)
                .width(Length::Fill),
        );
        rows.push(key_hints(&[
            ("Up Down", "pick a row"),
            ("Left Right", "change it"),
            ("Enter", "start"),
            ("Esc", "quit"),
        ]));
        let card = panel(rows)
            .gap(theme.cell_height() * 0.6)
            .width(theme.columns(84.0));
        Element::column([card])
            .size(Length::Fill, Length::Fill)
            .align(Align::Center)
            .justify(Align::Center)
            .background(theme.background.clone())
    }

    fn update(&mut self, message: Message) -> Control {
        match message {
            Message::Pick(field, index) => {
                self.focus = field;
                self.problem = None;
                match field {
                    Field::Mode => self.mode = index,
                    Field::Side => self.side = index,
                    Field::Clock => self.clock = index,
                    Field::Set => self.set = index,
                    Field::Code | Field::Sound | Field::Begin => {}
                }
            }
            Message::Focus(field) => self.focus = field,
            Message::ToggleSound => self.sound = !self.sound,
            Message::Begin => return self.begin(),
        }
        Control::Redraw
    }

    fn input(&mut self, event: &InputEvent) -> Control {
        // we put a pasted code in the code field wherever the focus was
        if matches!(event, InputEvent::Paste(_)) && self.fields().contains(&Field::Code) {
            self.focus = Field::Code;
        }
        if self.focus == Field::Code {
            match self.code.handle(event) {
                Edit::Submitted => return self.begin(),
                Edit::Changed | Edit::Moved => {
                    self.problem = None;
                    return Control::Redraw;
                }
                Edit::Ignored => {}
            }
        }
        let InputEvent::Key(key, mods) = event else {
            return Control::Wait;
        };
        let typing = self.focus == Field::Code;
        match intent(*key, mods.shift, typing) {
            Some(Intent::Quit) => return Control::Quit,
            Some(Intent::Begin) => return self.begin(),
            Some(Intent::Row(forward)) => self.step_focus(forward),
            Some(Intent::Value(forward)) => self.nudge(forward),
            None => return Control::Wait,
        }
        Control::Redraw
    }

    fn theme(&self, cell: CellPixels) -> Theme {
        let mut theme = Theme::dark(cell);
        if let Some(gold) = Rgba::from_hex("#e8b45c") {
            theme.accent = gold;
        }
        if let Some(ink) = Rgba::from_hex("#1a1208") {
            theme.on_accent = ink;
        }
        theme
    }
}

#[cfg(test)]
mod tests {
    use katui::proto::input::Mods;

    use super::*;

    fn menu() -> (Menu, Rc<RefCell<Option<Choice>>>) {
        let chosen = Rc::new(RefCell::new(None));
        let menu = Menu {
            mode: 0,
            side: 0,
            clock: 0,
            set: 0,
            sound: true,
            code: TextField::default(),
            focus: Field::Mode,
            problem: None,
            chosen: Rc::clone(&chosen),
        };
        (menu, chosen)
    }

    fn press(menu: &mut Menu, keys: &[Key]) -> Control {
        let mut last = Control::Wait;
        for key in keys {
            last = menu.input(&InputEvent::Key(*key, Mods::NONE));
        }
        last
    }

    #[test]
    fn the_keyboard_sets_up_a_bot_game_as_black_with_a_clock() {
        let (mut menu, chosen) = menu();
        let done = press(
            &mut menu,
            &[
                Key::Down,
                Key::Right,
                Key::Down,
                Key::Right,
                Key::Right,
                Key::Right,
                Key::Enter,
            ],
        );
        assert_eq!(done, Control::Quit);
        let choice = chosen.take().unwrap();
        assert_eq!(choice.start, Start::Bot(Some(Side::Black)));
        assert_eq!(choice.control, "5+3".parse().ok());
        assert!(!choice.muted);
    }

    #[test]
    fn joining_needs_a_valid_code_and_q_types_into_it() {
        let (mut menu, chosen) = menu();
        press(&mut menu, &[Key::Right, Key::Right, Key::Down]);
        assert_eq!(menu.focus, Field::Code);
        assert_eq!(press(&mut menu, &[Key::Enter]), Control::Redraw);
        assert!(menu.problem.is_some());
        for letter in "7FQ2K9XM3TBQ".chars() {
            press(&mut menu, &[Key::Char(letter.to_ascii_lowercase())]);
        }
        assert_eq!(menu.code.text(), "7fq2k9xm3tbq");
        assert_eq!(press(&mut menu, &[Key::Enter]), Control::Quit);
        assert!(matches!(
            chosen.take().map(|choice| choice.start),
            Some(Start::Join(_))
        ));
    }

    #[test]
    fn a_pasted_code_fills_the_code_field() {
        let (mut menu, chosen) = menu();
        press(&mut menu, &[Key::Right, Key::Right]);
        assert_eq!(menu.focus, Field::Mode);
        let pasted = InputEvent::Paste("7FQ2-K9XM-3TBQ".to_owned());
        assert_eq!(menu.input(&pasted), Control::Redraw);
        assert_eq!(menu.focus, Field::Code);
        assert_eq!(press(&mut menu, &[Key::Enter]), Control::Quit);
        assert!(matches!(
            chosen.take().map(|choice| choice.start),
            Some(Start::Join(_))
        ));
    }

    /// `just screenshots` writes the menu to docs/screenshots too.
    #[test]
    #[ignore = "writes the documentation screenshots"]
    fn menu_screenshot() {
        use katui::draw::{TextEngine, bundled_fonts};
        use katui::ui::{Ui, UiOptions};
        let (mut menu, _) = menu();
        press(&mut menu, &[Key::Down, Key::Down]);
        let cell = CellPixels {
            width: 10,
            height: 20,
        };
        let size = (1100, 560);
        let mut ui = Ui::new(
            TextEngine::new(bundled_fonts()).unwrap(),
            UiOptions::for_terminal(1, false),
        );
        let view = menu.theme(cell).scope(|_| menu.view());
        let picture = ui.render(&view, size, cell).flatten(cell, size).unwrap();
        let folder = std::env::var("KESS_SHOTS").unwrap_or_else(|_| "target/shots".into());
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(folder);
        std::fs::create_dir_all(&out).unwrap();
        image::save_buffer(
            out.join("menu.png"),
            picture.pixels(),
            picture.width(),
            picture.height(),
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    }

    #[test]
    fn rows_skip_what_a_mode_does_not_use() {
        let (mut menu, _) = menu();
        menu.mode = 3;
        assert_eq!(
            menu.fields(),
            [
                Field::Mode,
                Field::Clock,
                Field::Set,
                Field::Sound,
                Field::Begin
            ]
        );
        assert_eq!(press(&mut menu, &[Key::Char('q')]), Control::Quit);
    }
}
