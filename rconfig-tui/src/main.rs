use cargo_metadata::Message;
use clap::Parser;
use rconfig::{ConfigOption, JsonMap, Value, ValueType};
use std::{
    collections::BTreeMap,
    io::*,
    process::{exit, Command, Stdio},
};

use std::io;

use crossterm::ExecutableCommand;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, style::palette::tailwind, widgets::*};

struct Rconfig {
    crate_name: String,
    definition: String,
    features: String,
}

#[derive(clap::Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Ignore invalid configuration keys
    #[arg(long)]
    fix: bool,

    /// Don't ask when removing invalid configuration keys
    #[arg(long)]
    force: bool,

    /// Create a new empty `config.toml`
    #[arg(long)]
    init: bool,

    /// Features to be passed to the build
    #[arg(long)]
    features: Option<String>,

    /// Don't activate default features
    #[arg(long)]
    no_default_features: bool,
}

fn main() {
    let args = Args::parse();

    let cfg_path = std::path::PathBuf::from("./config.toml");

    let cfg_exists = if let Ok(metadata) = std::fs::metadata(&cfg_path) {
        if metadata.is_dir() {
            eprintln!("`config.toml` must be a file not a directory");
            exit(1);
        }
        true
    } else {
        false
    };

    // "fix" things by temporarily removing the config for the build - we need to restore the config before running the TUI
    // to keep the valid values
    if args.fix {
        if !cfg_exists {
            println!("No `config.toml` found. use `--init` to create a new one.");
            exit(1);
        }

        let mut new_file = cfg_path.clone();
        new_file.set_extension(".toml.old");
        std::fs::rename(&cfg_path, &new_file).unwrap();
    }

    let mut cargo_args = vec!["build".to_string(), "--message-format=json".to_string()];

    if let Some(features) = args.features {
        let features = format!("--features={}", features);
        cargo_args.push(features);
    }

    if args.no_default_features {
        cargo_args.push("--no-default-features".to_string());
    }

    let mut command = Command::new("cargo")
        .args(&cargo_args)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let reader = std::io::BufReader::new(command.stdout.take().unwrap());

    let mut per_crate_configs: Vec<Rconfig> = Vec::new();
    for message in cargo_metadata::Message::parse_stream(reader) {
        match message.unwrap() {
            Message::BuildScriptExecuted(script) => {
                let envs = script.env;
                let env_map: BTreeMap<_, _> =
                    envs.into_iter().map(|data| (data.0, data.1)).collect();

                if env_map.contains_key("__RCONFIG") {
                    let definition = env_map.get("__RCONFIG").unwrap().replace("%N%", "\n");
                    let crate_name = env_map.get("__RCONFIG_CRATE").unwrap().to_string();
                    let features = env_map.get("__RCONFIG_FEATURES").unwrap().to_string();

                    per_crate_configs.push(Rconfig {
                        crate_name,
                        definition,
                        features,
                    });
                }
            }
            _ => (), // don't care
        }
    }

    let exit_status = command.wait().expect("Couldn't get cargo's exit status");
    if !exit_status.success() {
        eprintln!("\n\nA successful build is needed");
        exit(1);
    }

    if args.fix {
        let mut new_file = cfg_path.clone();
        new_file.set_extension(".toml.old");
        std::fs::rename(&new_file, &cfg_path).unwrap();
    }

    if args.init {
        if (cfg_exists && (args.force || ask_confirm("Overwrite the current `config.toml`? (Y/N)")))
            || !cfg_exists
        {
            std::fs::write(&cfg_path, "").expect("Unable to create `config.toml`");
        }
    }

    let input = std::fs::read_to_string(cfg_path).expect("`config.toml` missing or not readable");

    // to avoid the need to check things everywhere just make sure the input contains entries for all contained crates
    let mut input_toml = basic_toml::from_str::<Value>(&input).unwrap();
    let input_toml = input_toml.as_object_mut().unwrap();
    for cfg in &per_crate_configs {
        if !input_toml.contains_key(&cfg.crate_name) {
            input_toml.insert(
                cfg.crate_name.clone(),
                rconfig::Value::Object(JsonMap::new()),
            );
        }
    }
    let input = basic_toml::to_string(input_toml).unwrap();

    // prepare repository
    let mut all_data: BTreeMap<String, (BTreeMap<String, ConfigOption>, Vec<String>)> =
        BTreeMap::new();
    for cfg in per_crate_configs {
        let definition = std::fs::read_to_string(cfg.definition).unwrap();
        let config = rconfig::parse_definition_str(&definition);
        all_data.insert(
            cfg.crate_name,
            (
                config,
                cfg.features.split(",").map(|v| v.to_string()).collect(),
            ),
        );
    }
    let repository = Repository::new(all_data, input);

    // TUI stuff ahead
    let terminal = init_terminal().unwrap();

    // create app and run it
    App::new(repository).run(terminal).unwrap();

    restore_terminal().unwrap();
}

fn ask_confirm(question: &str) -> bool {
    println!("{}", question);
    loop {
        let mut input = [0];
        let _ = std::io::stdin().read(&mut input);
        match input[0] as char {
            'y' | 'Y' => return true,
            'n' | 'N' => return false,
            _ => (),
        }
    }
}

struct Repository {
    data: BTreeMap<String, (BTreeMap<String, ConfigOption>, Vec<String>)>,
    user_cfg: String,
    path: Vec<String>,
}

impl Repository {
    pub fn new(
        data: BTreeMap<String, (BTreeMap<String, ConfigOption>, Vec<String>)>,
        user_cfg: String,
    ) -> Self {
        Self {
            data,
            user_cfg,
            path: Vec::new(),
        }
    }

    fn create_config(&self) -> String {
        let mut out = String::new();

        for (crate_name, (crate_config, crate_features)) in &self.data {
            let crate_features: Vec<&str> =
                crate_features.into_iter().map(|v| v.as_str()).collect();

            let crate_config = rconfig::evaluate_config_str_to_cfg(
                &self.user_cfg,
                &crate_name,
                crate_config.clone(),
                crate_features.clone(),
            )
            .unwrap();

            out.push_str(&format!("[{crate_name}]"));
            out.push_str("\n");

            let cfgs =
                rconfig::current_config_values(crate_config, crate_features.clone()).unwrap();
            for (name, value) in cfgs {
                out.push_str(&format!("{name}={value}"));
                out.push_str("\n");
            }
        }
        out
    }

    fn current(&self) -> BTreeMap<String, ConfigOption> {
        let crate_name = &self.path[0];
        let current = &(self.data[crate_name]).0;
        let features = self.current_features();
        let features = features.into_iter().map(|v| v.as_str()).collect();
        let config = rconfig::evaluate_config_str_to_cfg(
            &self.user_cfg,
            &crate_name,
            current.clone(),
            features,
        )
        .unwrap();

        let mut current = &config;

        for path_elem in &self.path[1..] {
            current = current.get(path_elem).unwrap().options.as_ref().unwrap();
        }
        current.clone()
    }

    fn current_features(&self) -> &Vec<String> {
        &(self.data[&self.path[0]]).1
    }

    pub fn get_current_level(&self) -> Vec<String> {
        let mut res = Vec::new();

        if self.path.is_empty() {
            for (item, _) in &self.data {
                res.push(item.to_string());
            }
        } else {
            let current = self.current();
            for (item, _) in current {
                res.push(item.to_string());
            }
        }

        res
    }

    pub fn get_current_level_desc(&self) -> Vec<String> {
        let mut res = Vec::new();

        if self.path.is_empty() {
            for (item, _) in &self.data {
                res.push(item.to_string());
            }
        } else {
            let current = self.current();
            for (_item, option) in current {
                let values = &option.values;
                let current_value = if let Some(value) = &option.__value {
                    format!("({})", Self::display_value(value, values))
                } else if let Some(value) = &option.default_value {
                    format!("(DEFAULT = {})", Self::display_value(value, values))
                } else {
                    String::new()
                };

                res.push(
                    format!("{} {}", option.description.to_string(), current_value).to_string(),
                );
            }
        }

        res
    }

    fn display_value(value: &rconfig::Value, values: &Option<Vec<rconfig::ValueItem>>) -> String {
        if values.is_none() {
            return value.to_string();
        } else {
            let display = values
                .as_ref()
                .unwrap()
                .iter()
                .find(|v| v.value == *value)
                .unwrap();
            return display.description.to_string();
        }
    }

    pub fn get_count(&self) -> usize {
        if self.path.is_empty() {
            self.data.len()
        } else {
            self.current().len()
        }
    }

    pub fn current_title(&self) -> String {
        if self.path.is_empty() {
            String::from("Root")
        } else {
            let mut title = self.path[0].clone();
            let mut current = &(self.data[&self.path[0]]).0;
            for path_elem in &self.path[1..] {
                title = current.get(path_elem).unwrap().description.clone();
                current = current.get(path_elem).unwrap().options.as_ref().unwrap();
            }
            title
        }
    }

    pub fn select(&mut self, select: usize) {
        let next = self
            .get_current_level()
            .into_iter()
            .enumerate()
            .find(|(index, _value)| *index == select)
            .unwrap()
            .1;
        self.path.push(next);
    }

    pub fn up(&mut self) {
        if !self.path.is_empty() {
            self.path.remove(self.path.len() - 1);
        }
    }

    pub fn is_value(&self, which: usize) -> bool {
        if self.path.is_empty() {
            false
        } else {
            let next = self
                .get_current_level()
                .into_iter()
                .enumerate()
                .find(|(index, _value)| *index == which)
                .unwrap()
                .1;

            self.current()
                .get(&next)
                .as_ref()
                .unwrap()
                .options
                .is_none()
        }
    }

    pub fn get_option(&self, which: usize) -> Option<ConfigOption> {
        if self.path.is_empty() {
            None
        } else {
            let next = self
                .get_current_level()
                .into_iter()
                .enumerate()
                .find(|(index, _value)| *index == which)
                .unwrap()
                .1;

            Some((*self.current().get(&next).as_ref().unwrap()).clone())
        }
    }

    pub fn set_value(&mut self, which: usize, value: rconfig::Value) {
        let next = self
            .get_current_level()
            .into_iter()
            .enumerate()
            .find(|(index, _value)| *index == which)
            .unwrap()
            .1;

        let mut cfg = basic_toml::from_str::<rconfig::Value>(&self.user_cfg).unwrap();

        let crate_cfg = cfg.as_object_mut().unwrap().get_mut(&self.path[0]).unwrap();
        let mut item = crate_cfg;
        for path_elem in &self.path[1..] {
            if !item
                .as_object_mut()
                .unwrap()
                .contains_key(path_elem.as_str())
            {
                item.as_object_mut().unwrap().insert(
                    path_elem.to_string(),
                    rconfig::Value::Object(Default::default()),
                );
            }
            item = item
                .as_object_mut()
                .unwrap()
                .get_mut(path_elem.as_str())
                .unwrap();
        }

        if item.as_object_mut().unwrap().contains_key(&next) {
            item.as_object_mut().unwrap().remove(&next);
        }

        item.as_object_mut().unwrap().insert(next, value);

        self.user_cfg = basic_toml::to_string(&cfg).unwrap();
    }
}

const TODO_HEADER_BG: Color = tailwind::BLUE.c950;
const NORMAL_ROW_COLOR: Color = tailwind::SLATE.c950;
const SELECTED_STYLE_FG: Color = tailwind::BLUE.c300;
const TEXT_COLOR: Color = tailwind::SLATE.c200;

fn init_terminal() -> Result<Terminal<impl Backend>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum InputMode {
    Number,
    Chars,
}

struct App {
    state: ListState,
    repository: Repository,

    show_input: bool,

    input: String,
    input_mode: InputMode,
    cursor_position: usize,

    cursor: Option<(u16, u16)>,
}

impl App {
    fn new(repository: Repository) -> Self {
        let mut initial_state = ListState::default();
        initial_state.select(Some(0));
        Self {
            repository,
            state: initial_state,
            show_input: false,
            input: "".to_string(),
            input_mode: InputMode::Chars,
            cursor_position: 0,
            cursor: None,
        }
    }
}

impl App {
    fn run(&mut self, mut terminal: Terminal<impl Backend>) -> io::Result<()> {
        loop {
            self.draw(&mut terminal)?;

            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    use KeyCode::*;

                    if !self.show_input {
                        match key.code {
                            Char('q') | Esc => return Ok(()),
                            Char('h') | Left => {
                                self.repository.up();
                                self.state.select(Some(0));
                                self.show_input = false;
                            }
                            Char('l') | Right | Enter => {
                                let selected = self.state.selected().unwrap_or_default();
                                if self.repository.is_value(selected) {
                                    let option = self.repository.get_option(selected);
                                    if let Some(option) = option {
                                        if let Some(value_type) = option.value_type {
                                            if value_type == ValueType::Bool {
                                                let current_value = option
                                                    .__value
                                                    .unwrap_or(option.default_value.unwrap())
                                                    .as_bool()
                                                    .unwrap();
                                                self.repository.set_value(
                                                    selected,
                                                    rconfig::Value::Bool(!current_value),
                                                )
                                            } else if value_type == ValueType::Enum {
                                                let current_value = option
                                                    .__value
                                                    .unwrap_or(option.default_value.unwrap())
                                                    .as_str()
                                                    .unwrap()
                                                    .to_owned();

                                                let values = option.values.as_ref().unwrap();
                                                let index = &values
                                                    .into_iter()
                                                    .enumerate()
                                                    .find(|v| v.1.value == current_value)
                                                    .unwrap()
                                                    .0;
                                                let index = (index + 1) % &values.len();

                                                self.repository.set_value(
                                                    selected,
                                                    rconfig::Value::String(
                                                        values[index].value.to_string(),
                                                    ),
                                                )
                                            } else {
                                                self.input_mode = if value_type == ValueType::U32 {
                                                    InputMode::Number
                                                } else {
                                                    InputMode::Chars
                                                };

                                                let default = if value_type == ValueType::U32 {
                                                    Value::Number(0.into())
                                                } else {
                                                    Value::String("".to_string())
                                                };

                                                self.show_input = true;
                                                self.input = option
                                                    .__value
                                                    .as_ref()
                                                    .unwrap_or(&default)
                                                    .to_string(); // TODO: this formats strings as \"str\"
                                                self.cursor_position = self.input.len()
                                            }
                                        }
                                    }
                                } else {
                                    self.repository
                                        .select(self.state.selected().unwrap_or_default());
                                    self.state.select(Some(0));
                                }
                            }
                            Char('j') | Down => {
                                if self.state.selected().unwrap_or_default()
                                    < self.repository.get_count() - 1
                                {
                                    self.state.select(Some(
                                        self.state.selected().unwrap_or_default() + 1,
                                    ));
                                }
                            }
                            Char('k') | Up => {
                                if self.state.selected().unwrap_or_default() > 0 {
                                    self.state.select(Some(
                                        self.state.selected().unwrap_or_default() - 1,
                                    ));
                                }
                            }
                            Char('s') => {
                                let cfg = self.repository.create_config();
                                std::fs::write("./config.toml", cfg).unwrap();
                                return Ok(());
                            }
                            _ => {}
                        }
                    } else {
                        // input mode key handling
                        // TODO can we use something like https://crates.io/crates/ratatui_input/ instead ?
                        match key.code {
                            Esc => {
                                self.show_input = false;
                                self.cursor = None;
                            }
                            Backspace => {
                                if self.cursor_position > 0 {
                                    self.input.remove(self.cursor_position - 1);
                                    self.cursor_position -= 1;
                                }
                            }
                            Left => {
                                if self.cursor_position > 0 {
                                    self.cursor_position -= 1;
                                }
                            }
                            Right => {
                                if self.cursor_position < self.input.len() {
                                    self.cursor_position += 1;
                                }
                            }
                            Enter => {
                                let selected = self.state.selected().unwrap_or_default();
                                if self.repository.is_value(selected) {
                                    let option = self.repository.get_option(selected);

                                    if let Some(option) = option {
                                        match option.value_type {
                                            Some(vt) => match vt {
                                                ValueType::U32 => {
                                                    let val = (self.input.parse::<u32>()).unwrap();
                                                    self.repository.set_value(
                                                        selected,
                                                        rconfig::Value::Number(val.into()),
                                                    );
                                                }
                                                ValueType::String => {
                                                    let val = self.input.clone();
                                                    self.repository.set_value(
                                                        selected,
                                                        rconfig::Value::String(val),
                                                    );
                                                }
                                                _ => (),
                                            },
                                            None => (),
                                        }
                                    }
                                    self.show_input = false;
                                    self.cursor = None;
                                }
                            }
                            KeyCode::Char(to_insert) => {
                                if self.input_mode == InputMode::Chars {
                                    self.input.insert(self.cursor_position, to_insert);
                                    self.cursor_position += 1;
                                } else if to_insert.is_numeric() {
                                    self.input.insert(self.cursor_position, to_insert);
                                    self.cursor_position += 1;
                                }
                            }
                            _ => (),
                        }
                    }
                }
            }
        }
    }

    fn draw(&mut self, terminal: &mut Terminal<impl Backend>) -> io::Result<()> {
        let cursor = self.cursor;

        terminal.draw(|f| {
            f.render_widget(self, f.size());

            if let Some((x, y)) = cursor {
                f.set_cursor(x, y);
            }
        })?;

        Ok(())
    }
}

impl Widget for &mut App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Create a space for header, todo list and the footer.
        let vertical = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(2),
        ]);
        let [header_area, rest_area, footer_area] = vertical.areas(area);

        // Create two chunks with equal vertical screen space. One for the list and the other for
        // the info block.
        let vertical = Layout::vertical([Constraint::Percentage(100)]);
        let [upper_item_list_area] = vertical.areas(rest_area);

        render_title(header_area, buf);
        self.render_item(upper_item_list_area, buf);
        render_footer(footer_area, buf);

        if self.show_input {
            let block = Block::bordered().title("Value");
            let mut area = centered_rect(60, 20, area);
            area.height = 3;
            block.render(area, buf);

            let text = Text::from(Line::from(self.input.clone()))
                .patch_style(Style::default().bg(Color::Gray).fg(Color::Black));
            area.y = area.y + area.height / 2;
            area.x = area.x + 2;
            area.width = area.width - 4;
            area.height = 1;
            text.render(area, buf);

            self.cursor = Some((area.x + self.cursor_position as u16, area.y));
        }
    }
}

impl App {
    fn render_item(&mut self, area: Rect, buf: &mut Buffer) {
        // We create two blocks, one is for the header (outer) and the other is for list (inner).
        let outer_block = Block::default()
            .borders(Borders::NONE)
            .fg(TEXT_COLOR)
            .bg(TODO_HEADER_BG)
            .title(self.repository.current_title())
            .title_alignment(Alignment::Center);
        let inner_block = Block::default()
            .borders(Borders::NONE)
            .fg(TEXT_COLOR)
            .bg(NORMAL_ROW_COLOR);

        // We get the inner area from outer_block. We'll use this area later to render the table.
        let outer_area = area;
        let inner_area = outer_block.inner(outer_area);

        // We can render the header in outer_area.
        outer_block.render(outer_area, buf);

        // Iterate through all elements in the `items` and stylize them.
        let items: Vec<ListItem> = self
            .repository
            .get_current_level_desc()
            .into_iter()
            .map(|v| ListItem::new(v))
            .collect();

        // Create a List from all list items and highlight the currently selected one
        let items = List::new(items)
            .block(inner_block)
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::REVERSED)
                    .fg(SELECTED_STYLE_FG),
            )
            .highlight_symbol(">")
            .highlight_spacing(HighlightSpacing::Always);

        // We can now render the item list
        // (look careful we are using StatefulWidget's render.)
        // ratatui::widgets::StatefulWidget::render as stateful_render
        StatefulWidget::render(items, inner_area, buf, &mut self.state);
    }
}

fn render_title(area: Rect, buf: &mut Buffer) {
    Paragraph::new("rconfig")
        .bold()
        .centered()
        .render(area, buf);
}

fn render_footer(area: Rect, buf: &mut Buffer) {
    Paragraph::new(
        "\nUse ↓↑ to move, ← to go up, → to go deeper or change the value, s/S to save and exit",
    )
    .centered()
    .render(area, buf);
}

/// helper function to create a centered rect using up certain percentage of the available rect `r`
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
