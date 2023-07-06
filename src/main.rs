#![feature(async_closure)]
#![feature(let_chains)]
#![feature(const_for)]
#![feature(result_option_inspect)]

use ahash::AHasher;
use interpolator::{format, Formattable};
use rfd::{FileDialog, MessageDialog, MessageLevel, MessageButtons};

use iced::{
    keyboard::{self, Modifiers},
    subscription, Event, Subscription,
};

use iced_aw::{
    menu::{MenuBar, MenuTree},
    menu_bar, menu_tree,
    native::menu_tree,
    style::menu_bar,
    Modal,
};

use rbl_circular_buffer::*;
use indexmap::{IndexMap, IndexSet, Equivalent};

use std::{collections::HashMap, net::ToSocketAddrs, time::Duration, path::{PathBuf, Path}, fmt::Display, sync::atomic::AtomicU32, hash::Hash};
use std::{iter::once, thread};
use tokio::io::AsyncReadExt;
use tokio::time::timeout;
use tokio_retry::Action;

use st3::fifo::Worker;
use std::{
    fs::{self, File, OpenOptions},
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use serde::{Deserialize, Serialize};

use serde_hex::{SerHexOpt, Compact};

use circular_buffer::CircularBuffer;
use std::io::Read;
use std::io::Write;

use rayon::prelude::*;
use standard_paths::LocationType;

use iced::{
    alignment, color, executor,
    theme::{self},
    widget::{
        button::{Appearance, StyleSheet},
        checkbox, column, container, row, scrollable, text, text_input, Column, Text,
    },
    Alignment, Application, Background, Command, Element, Font, Length, Theme,
};
use iced_lazy::lazy;

use iced::widget::button;

use iced_aw::card::Card;
use iced_aw::style::card::CardStyles;

use async_ssh2_lite::AsyncSession;
use async_ssh2_lite::{session_stream::AsyncSessionStream, SessionConfiguration};

type AIndexMap<K, V> = IndexMap<K, V, AHasher>;

use lazy_static::lazy_static;
use regex::{Regex, SubCaptureMatches};

use chrono::{Local, DateTime};

lazy_static! {
    static ref RE: Regex = Regex::new(r"\{(\w*\#?\w*)(.*)\}").unwrap();
}

use qcell::{TCell, TCellOwner};
struct Marker;
type ACell<T> = TCell<Marker, T>;
type ACellOwner = TCellOwner<Marker>;

const AUTH_FAILED: char = '\u{e99a}';
const DISCONNECTED: char = '\u{f239}';
const CONNECTED: char = '\u{e157}';
const CONNECTION_FAILED: char = '\u{e1cd}';
const EXECUTING: char = '\u{ef64}';
const SUCCESSFUL: char = '\u{e877}';
const ERROR: char = '\u{e000}';
const UNKNOWN: char = '\u{e000}';
const DELETE: char = '\u{e872}';
const VIEW_LOG: char = '\u{f1c3}';
const VIEW_ERROR_LOG: char = '\u{e87f}';
const ADD_ACTION: char = '\u{e146}';
const SELECT: char = '\u{f74d}';
const DESELECT: char = '\u{ebb6}';
const DONE: char = '\u{e876}';
const SCREENSHOT: char = '\u{ec08}';

#[derive(Debug, Clone)]
pub enum ConfigAction {
    Save,
    Export,
    Import,
    Open,
}

#[derive(Debug, Clone)]
pub enum Message {
    ShowDialog(Dialog),
    DialogInput(usize, String),
    AddrInput(String),
    PortInput(String),
    ActionAdd,
    AddrAdd,
    AddrDel(usize),
    AddrDelAll,
    AddrDeselAll,
    UpdateAddrSel(bool, usize),
    Action(ActionState, usize, bool),
    SelectAction,
    DelectAction,
    Config(ConfigAction),
    None,
}

#[derive(Debug, Clone)]
pub enum Dialog {
    Main,
    AddAction,
    ArgumentsForAction(usize, ArgumentsMap),
    Logs(usize, bool),
}

#[derive(Deserialize, Serialize)]
struct Client {
    address: Arc<str>,
    port: u16,
    #[serde(skip)]
    selected: AtomicBool,
    #[serde(skip, default = "default_cell")]
    connection: Arc<ACell<Option<AsyncSession<async_ssh2_lite::TokioTcpStream>>>>,
    #[serde(skip, default = "default_log")]
    output: ACell<CircularBuffer<1024, Log>>,
    #[serde(skip, default = "default_state")]
    state: AtomicU32,
}

impl Hash for Client {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.address.hash(state);
        self.port.hash(state);
    }
}

impl PartialEq for Client {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address && self.port == other.port
    }
}

impl Eq for Client {}

#[derive(Deserialize, Serialize, Hash, PartialEq, Eq)]
struct Addr
{
    address: Arc<str>,
    port: u16,
}


impl Equivalent<Addr> for Client {
    fn equivalent(&self, key: &Addr) -> bool {
        self.address == key.address && self.port == key.port
        
    }
}


fn default_cell() -> Arc<ACell<Option<AsyncSession<async_ssh2_lite::TokioTcpStream>>>> {
    Arc::new(ACell::new(None))
}

const fn default_log() -> ACell<CircularBuffer<1024, Log>> {
    ACell::new(CircularBuffer::new())
}

fn default_state() -> AtomicU32 {
   AtomicU32::new(DISCONNECTED.into())
}

impl Default for Client {
    fn default() -> Self {
        Self {
            address: "".into(),
            port: 22,
            selected: Default::default(),
            connection: default_cell(),
            output: default_log(),
            state: default_state(),
        }
        
    }
}

struct Log {
    stdout: String,
    stderr: String,
    timestamp: DateTime<Local>,
}


struct ActionStyle;

impl StyleSheet for ActionStyle {
    type Style = theme::Theme;

    fn active(&self, _: &Self::Style) -> Appearance {
        Appearance {
            shadow_offset: iced::Vector { x: 8.0, y: 8.0 },
            background: None,
            border_radius: 8.0,
            border_width: 2.0,
            border_color: color!(0, 0, 0),
            text_color: color!(1, 1, 1),
        }
    }
}

struct ActionStyleSelection;

impl StyleSheet for ActionStyleSelection {
    type Style = theme::Theme;

    fn active(&self, _: &Self::Style) -> Appearance {
        Appearance {
            shadow_offset: iced::Vector { x: 8.0, y: 8.0 },
            background: Some(Background::Color(color!(100, 100, 255))),
            border_radius: 8.0,
            border_width: 2.0,
            border_color: color!(100, 100, 255),
            text_color: color!(255, 255, 255),
        }
    }
}


type ArgumentsMap = Arc<IndexSet<String>>;
#[derive(Deserialize, Serialize)]
pub struct ActionDesc {
    description: Arc<str>,
    command: String,
    // #[serde(with = "SerHexOpt::<Compact>")]
    logo: Option<u32>,
    #[serde(skip)]
    arguments: ArgumentsMap, 
    #[serde(skip, default="atomicbool_default")]
    selected: AtomicBool,
}

fn atomicbool_default() -> AtomicBool {
    AtomicBool::new(true)
}

impl Hash for ActionDesc {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.description.hash(state);
        self.command.hash(state);
    }
}

impl PartialEq for ActionDesc {
    fn eq(&self, other: &Self) -> bool {
        self.description == other.description && self.logo == other.logo 
    }
}

impl Eq for ActionDesc {}

impl ActionDesc {
    fn new(description : Arc<str>, command: String, logo: Option<u32>, selected: AtomicBool) -> Self {
        let args =  Self::generate_args(&command);
Self {
            description,
            command, 
            logo,
            selected,
            arguments: args,
        }
       
    }

    fn generate_args(command: &str) -> ArgumentsMap {
        let args = 
            Arc::new(RE
                    .captures_iter(&command)
                    .filter_map(|caps| {
                        caps.get(1).map(|name| {
                            name.as_str().to_owned()
                        })
                    })
                    .collect());       
        args
        
    }
}

#[derive(Debug, Clone)]
pub enum ActionState {
    Selection,
    NeedArguments,
    None,
}

#[derive(Debug)]
enum ConfigError {
    IO(std::io::ErrorKind),
    Toml(toml::de::Error),
}

impl From<std::io::Error> for ConfigError {
    fn from(value: std::io::Error) -> Self {
        Self::IO(value.kind())
    }
}
impl From<toml::de::Error> for ConfigError {
    fn from(value: toml::de::Error) -> Self {
        Self::Toml(value)
    }
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::IO(io) =>  write!(f, "IO Error : {}", io),
            ConfigError::Toml(toml) => write!(f, "Toml Parsing Error : {}", toml),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Config {
    clients: IndexSet<Arc<Client>>,
    actions: Vec<ActionDesc>,
    password: Arc<str>,
    user: Arc<str>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            clients: [].into(),
            actions: [].into(),
            password: "".into(),
            user: "".into(),
        }
    }
}

#[derive(Default)]
struct DiskConfig(Config);

#[derive(Default)]
struct MemConfig(Config);

impl Deref for DiskConfig {
    type Target = Config;
    
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for MemConfig{
    type Target = Config;
    
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for MemConfig{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// impl From<&MemConfig> for &DiskConfig {
//     fn from(data: &MemConfig) -> Self {
//         &DiskConfig(&data.0)
//     }
// }

impl DiskConfig {
    fn load(path: &Path) -> Result<Self, ConfigError> {

        let mut handle = OpenOptions::new()
                .write(true)
                .read(true)
                .open(path)?;      

        let mut conf = String::new();
        handle.read_to_string(&mut conf)?;
        toml::from_str(&conf).map_err(|e| ConfigError::Toml(e)).map(|c| DiskConfig(c))
    }
        
}

impl From<DiskConfig> for MemConfig {
    fn from(data: DiskConfig) -> Self {

        let DiskConfig(mut conf) = data;

        conf.actions.iter_mut().for_each(|a| {
            a.arguments = ActionDesc::generate_args(&a.command);
        });

        MemConfig(conf)

    }
}


lazy_static! {
    static ref CONFIG_PATH: std::path::PathBuf = {
        let sp = standard_paths::default_paths!()
            .writable_location(LocationType::AppConfigLocation)
            .expect("Unable to get configuration path");
        fs::create_dir_all(&sp).expect("Unable to create inital configuration path");
        sp.join("config.toml")
    };
}

fn save(data: &MemConfig, path: Option<&Path>) {
    File::create(path.unwrap_or(CONFIG_PATH.as_path()))
        .expect("Unable to create configuration file")
        .write(
            toml::to_string(data.deref())
                .expect("Unable to generate default toml configuration")
                .as_bytes(),
        )
        .expect("Unable to write to new configuration file");
}

struct Data {
    config: MemConfig,

    clients_version: AtomicUsize,
    actions_version: AtomicUsize,
    select_all: AtomicBool,
    // show_log: Option<CircularBuffer<16384, u8>>,
    client_addr_input: String,
    client_port_input: String,
    select_actions: AtomicBool,

    current_dialog: Dialog,
    dialog_inputs: Vec<String>,
}

impl Default for Data {
    fn default() -> Self {
        Data {
            config: Default::default(),
            client_addr_input: String::new(),
            client_port_input: "22".into(),
            select_all: AtomicBool::new(false),
            // show_log: None,
            select_actions: AtomicBool::new(false),
            clients_version: AtomicUsize::new(0),
            actions_version: AtomicUsize::new(0),

            current_dialog: Dialog::Main,
            dialog_inputs: vec!["".to_owned(); 1024],
        }
    }
}

impl Application for Data {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    type Theme = Theme;

    fn new(_flags: ()) -> (Data, Command<Self::Message>) {
        let diskconfig = DiskConfig::load(CONFIG_PATH.as_path()).map_or_else(|e| {
                if MessageDialog::new().set_level(MessageLevel::Warning).set_title(e.to_string().as_str()).set_description("Do you want to load default configuration?").set_buttons(MessageButtons::YesNo).show()  {
                     Default::default()
                    } else {
                        panic!("No way of getting valid configuration")
                    }                    
        }, |v| v);

        (
            Data {
                config: diskconfig.into(),
                ..Default::default()
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("TV Player Control Contre")
    }

    fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
        match message {
            Message::ShowDialog(dialog) => {
                self.current_dialog = dialog;
            }
            Message::DialogInput(index, input) => {
                self.dialog_inputs[index] = input;
            }
            Message::Config(action) => {
                match action {
                    ConfigAction::Save => save(&self.config, None),
                    ConfigAction::Export =>  {
                        if let Some(file) = FileDialog::new()
                            .add_filter("Configuration", &["toml"])
                            .save_file() {
                        
                        save(&self.config, Some(file.as_path()));
                    }}
                    ConfigAction::Open => {open::that(CONFIG_PATH.as_os_str()).ok();},
                    ConfigAction::Import => {
                        if let Some(file) = FileDialog::new()
                            .add_filter("Configuration", &["toml"])
                            .pick_file() {
                            match  DiskConfig::load(file.as_path()) {
                                Ok(config) =>  {self.config = config.into();},
                                Err(e) =>  {MessageDialog::new().set_level(MessageLevel::Error).set_title("Unable to load config").set_description(e.to_string().as_str()).show();}
                            }                    
                        
                    }
                        }
                }
            }
            Message::UpdateAddrSel(state, index) => {
                if let Some(v) = self.config.clients.get_index(index) {
                    self.clients_version.fetch_add(1, Ordering::Relaxed);
                    v.selected.store(state, Ordering::Relaxed);
                }
            }
            Message::DelectAction => {
                self.actions_version.fetch_add(1, Ordering::Relaxed);
                if self.select_actions.load(Ordering::Relaxed) {
                    self.config.actions
                        .retain(|a| !a.selected.load(Ordering::Relaxed));
                }
            }
            Message::ActionAdd => {
                self.config.actions.push(
                    ActionDesc::new(Arc::from(self.dialog_inputs[1].as_str()), self.dialog_inputs[2].to_owned(), u32::from_str_radix(&self.dialog_inputs[0], 16).ok(), atomicbool_default())                );
                self.actions_version.fetch_add(1, Ordering::Relaxed);
            }
            Message::SelectAction => {
                self.actions_version.fetch_add(1, Ordering::Relaxed);
                self.select_actions.fetch_xor(true, Ordering::Relaxed);
            }
            Message::AddrInput(addr) => {
                self.client_addr_input = addr;
            }
            Message::PortInput(port) => {
                if port.parse::<u16>().is_ok() {
                    self.client_port_input = port;
                }
            }
            Message::AddrAdd => {
                if !self.client_addr_input.is_empty() {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                    self.config.clients.insert(
                        Arc::new(Client {
                            address: Arc::from(self.client_addr_input.clone()),
                            port: self.client_port_input.parse().unwrap_or(22),
                            ..Default::default()
                            
                        })
                    );
                }
            }

            Message::AddrDel(index) => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);

                // todo!();
                // self.config.clients.remove(&client);
            }
            Message::AddrDelAll => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.config.clients.clear();
            }
            Message::AddrDeselAll => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.config.clients
                    .iter()
                    .for_each(|c| c.selected.store(false, Ordering::Relaxed));
            }
            // Message::ActionTileSelect()
            Message::Action(state, index, decode) => {
                if let Some(a) = self.config.actions.get(index) {
                    match state {
                        ActionState::Selection => {
                            self.actions_version.fetch_add(1, Ordering::Relaxed);
                            a.selected.fetch_xor(true, Ordering::Relaxed);
                        }
                        ActionState::NeedArguments => {
                            self.current_dialog =
                                Dialog::ArgumentsForAction(index, a.arguments.clone());
                        }
                        ActionState::None => {
                            self.clients_version.fetch_add(1, Ordering::Relaxed);
                            let user = self.config.user.clone();
                            let password = self.config.password.clone();
                            let command = if decode {
                                format(
                                    a.command.as_ref(),
                                    &a.arguments.as_ref().into_iter().map(|a| a.as_str()).zip(self.dialog_inputs.iter().map(|a| Formattable::display(a))).collect::<HashMap::<_, _>>()
                                )
                                .unwrap()
                            } else {
                                a.command.deref().to_owned()
                            };
                            let clients: Vec<_> = self.config
                                .clients
                                .iter()
                                .filter(|c| {
                                    c.selected.load(Ordering::Relaxed)
                                        || self.select_all.load(Ordering::Relaxed)
                                }).cloned()
                                .collect();

                            return Command::perform(
                                async move {
                                    tokio_scoped::scope(|s| {
                                        for c in &clients {
                                            s.spawn(async {
                                                        let res: Result<_, async_ssh2_lite::Error> =
                                                            async {
                                                                for _ in 0..3 {
                                                                    let mut owner = ACellOwner::new();
                                                                    if let Some(ref session) = owner.ro(c.connection.deref()) {
                                                                        let mut channel = session.channel_session().await?;
                                                                        channel.exec(&command).await?;
                                                                        let mut stdout = String::new();
                                                                        let mut stderr = String::new();
                                                                        channel.read_to_string(&mut stdout).await?;
                                                                        channel.stderr().read_to_string(&mut stderr).await?;
                                                                        c.output.rw(&mut owner).push_back(Log { stdout, stderr, timestamp: chrono::offset::Local::now()});
                                                                        channel.close().await?;
                                                                        return Ok(if channel.exit_status()? == 0 {SUCCESSFUL} else {ERROR} );
                                                                    }
                                                                    else {
                                                                        let addr = (c.address.deref(), c.port).to_socket_addrs().map_err(|x| async_ssh2_lite::Error::Other(Box::new(x)))?.next().unwrap();

                                                                        let mut conf = SessionConfiguration::new();
                                                                        conf.set_timeout(5000);
                                                                    
                                                                        let mut conn =
                                                                            AsyncSession::<async_ssh2_lite::TokioTcpStream>::connect(
                                                                                addr,
                                                                                conf,
                                                                            )
                                                                            .await?;
                                                                        conn.handshake().await?;
                                                                        dbg!(conn.userauth_password(&user, &password).await)?;
                                                                        *c.connection.rw(&mut owner) = Some(conn);
                                                                    }
                                                                }

                                                                Ok(UNKNOWN)
                                                            }
                                                            .await;

                                                         let state = match dbg!(res) {
                                                            Ok(state) => state,
                                                            Err(err) => { use async_ssh2_lite::Error; match err {
                                                                Error::Ssh2(err)=> {
                                                                    AUTH_FAILED
                                                                }
                                                                _ => UNKNOWN,
                                                            }},
                                                        };
                                                c.state.store(state as u32, Ordering::Relaxed);


                                            });
                                        }
                                    });
                                },
                                |_| Message::None,
                            );
                        }
                    }
                }
            }
            Message::None => {}
        }
        Command::none()
    }


    fn view(&self) -> Element<Self::Message> {
        let menu_button = |label: &str, msg: Message|  menu_tree!(button(text(label).width(Length::Fill).height(Length::Fill).vertical_alignment(alignment::Vertical::Center)).on_press(msg));
        dbg!("Update");

        let menu = menu_bar!(menu_tree("Configuration", vec![menu_button("Save (Ctrl + S)", Message::Config(ConfigAction::Save)), menu_button("Import", Message::Config(ConfigAction::Import)), menu_button("Export", Message::Config(ConfigAction::Export)), menu_button("View", Message::Config(ConfigAction::Open))]));
        let clients = lazy(self.clients_version.load(Ordering::Relaxed), |_| {
            scrollable(
                container(Column::with_children(
                    self.config.clients
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            row![
                                checkbox(
                                    c.address.deref(),
                                    c.selected.load(Ordering::Relaxed)
                                        || self.select_all.load(Ordering::Relaxed),
                                    move |state| { Message::UpdateAddrSel(state, i) }
                                )
                                .width(Length::Fill),
                                text(c.port.to_string()).width(Length::Fill),
                                icon(
                                    unsafe {char::from_u32_unchecked(c.state.load(Ordering::Relaxed))}
                                    ,
                                    40
                                ),
                                button(icon(VIEW_LOG, 30))
                                    .on_press(Message::ShowDialog(Dialog::Logs(i, false)))
                                    .style(theme::Button::Text),
                                button(icon(SCREENSHOT, 30))
                                    // .on_press(Message::ShowDialog())
                                // button(icon(VIEW_ERROR_LOG, 30))
                                //     .on_press(Message::ViewLog(b.stderr))
                                //     .style(theme::Button::Text),
                                // button(icon(DELETE, 20))
                                //     .on_press(Message::AddrDel(a.clone()))
                                //     .padding(10)
                                //     .style(theme::Button::Destructive),
                            ]
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .padding(5)
                        })
                        .map(Element::from)
                        .collect(),
                ))
                .padding(10),
            )
            .width(Length::Fill)
            .height(400)
        });

        let input = row![
            button(icon(DESELECT, 20))
                .on_press(Message::AddrDeselAll)
                .padding(10)
                .style(theme::Button::Destructive),
            checkbox("Select All", self.select_all.load(Ordering::Relaxed), |b| {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.select_all.store(b, Ordering::Relaxed);
                Message::None
            }),
            text_input("Client Address", &self.client_addr_input)
                .width(Length::Fill)
                .on_input(Message::AddrInput)
                .on_submit(Message::AddrAdd),
            text_input("Port", &self.client_port_input)
                .width(Length::Fill)
                .on_input(|str| { Message::PortInput(str) })
                .on_submit(Message::AddrAdd),
            button(row![icon(DELETE, 20), "All"].spacing(5))
                .on_press(Message::AddrDelAll)
                .padding(10)
                .style(theme::Button::Destructive),
        ]
        .align_items(Alignment::Center)
        .spacing(20)
        .padding(5);

        let actions = lazy(self.actions_version.load(Ordering::Relaxed), |_| {
            iced_aw::Wrap::with_elements(
                self.config.actions
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let logo: Element<Self::Message> = if let Some(logo) = a.logo.map_or(None, |a| char::from_u32(a)){
                            icon(logo, 50).into()
                        } else {
                            row![].into()
                        };
                        container(
                            button(
                                container(
                                    column![
                                        logo,
                                        text(a.description.deref())
                                            .horizontal_alignment(alignment::Horizontal::Center)
                                            .vertical_alignment(alignment::Vertical::Center)
                                    ]
                                    .align_items(Alignment::Center),
                                )
                                .center_x()
                                .center_y()
                                .width(Length::Fill)
                                .height(Length::Fill),
                            )
                            .on_press(Message::Action(
                                if self.select_actions.load(Ordering::Relaxed) {
                                    ActionState::Selection
                                } else if a.arguments.len() > 0 {
                                    ActionState::NeedArguments
                                } else {
                                    ActionState::None
                                },
                                i,
                                false,
                            ))
                            .style(theme::Button::Custom(
                                if a.selected.load(Ordering::Relaxed)
                                    && self.select_actions.load(Ordering::Relaxed)
                                {
                                    Box::new(ActionStyleSelection)
                                } else {
                                    Box::new(ActionStyle)
                                },
                            ))
                            .width(200)
                            .height(200),
                        )
                        .center_x()
                        .center_y()
                        .padding(10)
                        .into()
                    })
                    .collect(),
            )
        });

        let mut add = vec![
            button(icon(ADD_ACTION, 30))
                .padding(10)
                .style(theme::Button::Secondary)
                .on_press(Message::ShowDialog(Dialog::AddAction))
                .into(),
            button(icon(SELECT, 30))
                .padding(10)
                .style(if self.select_actions.load(Ordering::Relaxed) {
                    theme::Button::Custom(Box::new(ActionStyleSelection))
                } else {
                    theme::Button::Secondary
                })
                .on_press(Message::SelectAction)
                .into(),
        ];
        if self.select_actions.load(Ordering::Relaxed) {
            add.insert(
                0,
                button(icon(DELETE, 20))
                    .on_press(Message::DelectAction)
                    .padding(10)
                    .style(theme::Button::Destructive)
                    .into(),
            );
        }
        let add = container(row(add).spacing(10).align_items(Alignment::Center))
            .width(Length::Fill)
            .align_x(alignment::Horizontal::Right);

        let content = column![clients, input, actions, add].spacing(10);
        // let content = column![menu, clients, input, actions, add].spacing(10);

        let view: Element<Message> = match self.current_dialog {
            Dialog::Main => content.into(),
            Dialog::Logs(index, show_error) => Modal::new(true, content, move || {
                let owner = ACellOwner::new();
                let output = if let Some(client) = self.config.clients.get_index(index) {
                    column(client.output.ro(&owner).iter().rev().map(|o| {
                        Card::new(text(o.timestamp.to_string()), text(if show_error { o.stderr.clone() } else {o.stdout.clone()}))
                    }).map(Element::from).collect())
                } else {
                    column![text("Unable to load logs")]
                };
                scrollable(
                    Card::new(
                        "Logs",
                        container(column![
                            checkbox("Show Error", show_error, move |state| Message::ShowDialog(
                                Dialog::Logs(index, state)
                            )),
                            output
                        ])
                        .width(Length::Fixed(500.0))
                        .padding(20)
                        .align_x(alignment::Horizontal::Center)
                        .align_y(alignment::Vertical::Center)
                        .center_x()
                        .center_y(),
                    )
                    .width(Length::Shrink)
                    .height(Length::Shrink)
                    .style(CardStyles::Secondary),
                )
                .into()
            })
            .on_esc(Message::ShowDialog(Dialog::Main))
            .into(),
            Dialog::AddAction => Modal::new(true, content, || {
                Card::new(
                    "New Action",
                    container(
                        Column::with_children(
                            [
                                ("Icon", "code point"),
                                ("Description", "command info"),
                                ("Command", "shell script"),
                            ]
                            .iter()
                            .enumerate()
                            .map(|(i, (title, tip))| {
                                row![
                                    text(title),
                                    text_input(tip, &self.dialog_inputs[i])
                                        .width(Length::Fill)
                                        .on_input(move |str| Message::DialogInput(i, str))
                                ]
                                .spacing(10)
                                .into()
                            })
                            .collect(),
                        )
                        .push(button("Add").on_press(Message::ActionAdd)),
                    )
                    .width(Length::Fixed(300.0))
                    .padding(20)
                    .align_x(alignment::Horizontal::Center)
                    .align_y(alignment::Vertical::Center)
                    .center_x()
                    .center_y(),
                )
                .width(Length::Shrink)
                .height(Length::Shrink)
                .style(CardStyles::Secondary)
                .into()
            })
            .on_esc(Message::ShowDialog(Dialog::Main))
            .into(),
            Dialog::ArgumentsForAction(index, ref arguments) => {
                Modal::new(true, content, move || {
                    Card::new(
                        "Arguments",
                        container(
                            Column::with_children(
                                arguments
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(i, a)| {
                                        let mut arg = a.split('#');
                                        arg.next().map(|a| {
                                        row![
                                            text(a),
                                            text_input(arg.next().unwrap_or(""), &self.dialog_inputs[i],)
                                                .width(Length::Fill)
                                                .on_input(move |str| Message::DialogInput(i, str))
                                        ]
                                        .spacing(20)
                                        })
                                    })
                                    .map(Element::from)
                                    .collect(),
                            )
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(
                                container(button("Run").on_press(Message::Action(
                                    ActionState::None,
                                    index,
                                    true,
                                )))
                                .align_x(alignment::Horizontal::Right),
                            ),
                        )
                        .width(Length::Fixed(300.0))
                        .padding(20)
                        .align_x(alignment::Horizontal::Center)
                        .align_y(alignment::Vertical::Center)
                        .center_x()
                        .center_y(),
                    )
                    .width(Length::Shrink)
                    .height(Length::Shrink)
                    .style(CardStyles::Secondary)
                    .into()
                })
                .on_esc(Message::ShowDialog(Dialog::Main))
                .into()
            }
        };

        let cont = scrollable(
            container(view)
                .width(Length::Fill)
                .padding(40)
                .align_x(iced::alignment::Horizontal::Center)
                .center_x(),
        );

        column![menu, cont]
        .into()
    }

    fn subscription(&self) -> iced::Subscription<Self::Message> {
        // match self.subscription_event {
        //     SEvent::ExecuteAction(ref _action) => {
        //         // self.subscription_event = SEvent::None;
        //         // subscription::unfold(, , )
        //         Subscription::none()
        //     }
        //     SEvent::None => Subscription::none(),
        // }
        subscription::events_with(|e, s| match (e, s) {
            (
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key_code: keyboard::KeyCode::S,
                    modifiers: Modifiers::CTRL,
                }),
                _,
            ) => Some(Message::Config(ConfigAction::Save)),

            _ => None,
        })
    }
}

const ICONS: Font = Font::External {
    name: "icons",
    bytes: include_bytes!("fonts/icons.ttf"),
};

fn icon(unicode: char, size: u16) -> Text<'static> {
    text(unicode.to_string())
        .font(ICONS)
        .size(size)
        .horizontal_alignment(alignment::Horizontal::Center)
}

#[tokio::main]
async fn main() -> iced::Result {
    Data::run(iced::Settings::default())
}
