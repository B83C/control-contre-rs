#![feature(async_closure)]
#![feature(let_chains)]
#![feature(const_for)]
#![feature(result_option_inspect)]
#![feature(inherent_associated_types)]

use async_trait::async_trait;
use russh::*;
use russh_keys::*;

use ahash::AHasher;
use async_mutex::Mutex;
use interpolator::{format, Formattable};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use tokio::{io::AsyncReadExt, time::timeout};

use iced::{
    clipboard,
    keyboard::{self, Modifiers},
    subscription,
    widget::{horizontal_space, image, pick_list::Handle, tooltip, tooltip::Position},
    Color, Event,
};

use iced_aw::{
    menu_bar, menu_tree,
    native::{menu_bar, menu_tree},
    Modal,
};

use indexmap::{Equivalent, IndexMap, IndexSet};
use smol_str::SmolStr;

use std::{
    borrow::Cow, collections::HashMap, fmt::Display, hash::Hash, net::ToSocketAddrs, path::Path,
    time::Duration,
};

use std::{
    fs::{self, File, OpenOptions},
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use serde::{Deserialize, Serialize};

use circular_buffer::CircularBuffer;
use std::io::Read;
use std::io::Write;

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

type AIndexMap<K, V> = IndexMap<K, V, AHasher>;

use lazy_static::lazy_static;
use regex::Regex;

use chrono::{DateTime, Local};

lazy_static! {
    static ref RE: Regex = Regex::new(r"\{([\w \#]*)?[^\}\{]*\}").unwrap();
}

use qcell::{QCell, QCellOwner, TCell, TCellOwner};

struct Marker;
type ACell<T> = TCell<Marker, T>;
type ACellOwner = TCellOwner<Marker>;

const AUTH_FAILED: char = '\u{e99a}';
const DISCONNECTED: char = '\u{f239}';
const CONNECTED: char = '\u{e157}';
const CONNECTION_FAILED: char = '\u{e1cd}';

const EXECUTING: char = '\u{ef64}';
const SUCCESSFUL: char = '\u{e877}';
const EXEC_ERR: char = '\u{e000}';
const ERROR: char = '\u{e001}';
const TIMED_OUT: char = '\u{e51d}';

const DELETE: char = '\u{e872}';
const REFRESH: char = '\u{e5d5}';
const EDIT: char = '\u{e3c9}';
const COPY: char = '\u{e14d}';
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
    UsernameInput(String),
    PasswordInput(String),
    AddAction(Option<usize>),
    AddrAdd,
    AddrDel(usize),
    AddrDelAll,
    AddrDeselAll,
    UpdateAddrSel(bool, usize),
    Action(bool, usize, bool),
    SelectAction,
    EditAction,
    DelectAction,
    Config(ConfigAction),
    Screenshot(usize, bool),
    RefreshAddr,
    CopyToClipboard(SmolStr),
    None,
}

use enum_index::EnumIndex;
use enum_index_derive::EnumIndex;
#[derive(Debug, Clone, EnumIndex)]
pub enum Dialog {
    Main,
    AddAction(Option<usize>),
    ArgumentsForAction(usize, ArgumentsMap),
    Logs(usize, bool),
    Screenshot(usize),
}

struct ClientHandler {}

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        self,
        _server_public_key: &key::PublicKey,
    ) -> Result<(Self, bool), Self::Error> {
        Ok((self, true))
    }
}

#[derive(Deserialize, Serialize)]
struct Client {
    address: SmolStr,
    port: u16,
    user: SmolStr,
    pass: SmolStr,
    #[serde(skip)]
    selected: AtomicBool,
    #[serde(skip)]
    info: Arc<Mutex<ClientInfo>>,
    #[serde(skip)]
    screenshot: Arc<Mutex<Option<Cow<'static, [u8]>>>>,
}

impl Client {
    async fn connect(&self) -> Result<client::Handle<ClientHandler>, russh::Error> {
        let config = client::Config {
            connection_timeout: Some(Duration::from_secs(10)),
            ..<_>::default()
        };

        let sh = ClientHandler {};
        let mut session =
            client::connect(Arc::new(config), (self.address.as_str(), self.port), sh).await?;
        let _auth_res = session
            .authenticate_password(self.user.as_ref(), self.pass.as_ref())
            .await?;

        Ok(session)
    }

    async fn exec(
        &self,
        info: &mut ClientInfo,
        command: &str,
        data: Option<&[u8]>,
        reply: bool,
    ) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>, u32), russh::Error> {
        async {
            let session = info.connection.get_or_insert(self.connect().await?);
            let mut channel = session.channel_open_session().await?;
            channel.exec(reply, command.as_bytes()).await?;
            if let Some(data) = data {
                channel.data(data.as_ref()).await?;
                channel.eof().await?;
            }
            let mut stdout = None;
            let mut stderr = None;
            while let Some(msg) = channel.wait().await {
                match msg {
                    russh::ChannelMsg::Data { ref data } => {
                        stdout
                            .get_or_insert_with(|| Vec::new())
                            .write_all(data)
                            .ok();
                    }
                    russh::ChannelMsg::ExtendedData { ref data, .. } => {
                        stderr
                            .get_or_insert_with(|| Vec::new())
                            .write_all(data)
                            .ok();
                    }

                    russh::ChannelMsg::ExitStatus { exit_status } => {
                        channel.close().await?;
                        return Ok((stdout, stderr, exit_status));
                    }
                    _ => {}
                }
            }
            channel.close().await?;
            return Err(russh::Error::ConnectionTimeout);
        }
        .await
    }

    async fn upload_file(
        &self,
        info: &mut ClientInfo,
        data: &[u8],
        path: &Path,
    ) -> Result<u32, russh::Error> {
        self.exec(
            info,
            ["scp", "/dev/stdin", path.to_string_lossy().as_ref()]
                .join(" ")
                .as_str(),
            Some(data),
            true,
        )
        .await
        .map(|(_, _, status)| status)
    }

    async fn download_file(
        &self,
        info: &mut ClientInfo,
        path: &Path,
    ) -> Result<(u32, Option<Vec<u8>>), russh::Error> {
        self.exec(
            info,
            ["scp", path.to_string_lossy().as_ref(), "/dev/stdout"]
                .join(" ")
                .as_str(),
            None,
            true,
        )
        .await
        .map(|(output, _, status)| (status, output))
    }
}

#[derive(Default)]
struct ClientInfo {
    connection: Option<client::Handle<ClientHandler>>,
    output: CircularBuffer<1024, Log>,
    status: char,
    msg: SmolStr,
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

impl Equivalent<(SmolStr, u16)> for Client {
    fn equivalent(&self, key: &(SmolStr, u16)) -> bool {
        self.address == key.0 && self.port == key.1
    }
}

impl Default for Client {
    fn default() -> Self {
        Self {
            address: "".into(),
            port: 22,
            selected: Default::default(),
            user: SmolStr::new(""),
            pass: SmolStr::new(""),
            info: Arc::new(Mutex::new(Default::default())),
            // mutex: Arc::new(Mutex::new(ACellOwner::new())),
            screenshot: Arc::new(Mutex::new(None)),
        }
    }
}

struct Log {
    stdout: Option<SmolStr>,
    stderr: Option<SmolStr>,
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

type ArgumentsMap = Arc<IndexSet<SmolStr>>;
#[derive(Deserialize, Serialize)]
pub struct ActionDesc {
    description: SmolStr,
    command: SmolStr,
    timeout: u64,
    // #[serde(with = "SerHexOpt::<Compact>")]
    logo: Option<u32>,
    need_reply: bool,
    #[serde(skip)]
    arguments: ArgumentsMap,
    #[serde(skip, default = "atomicbool_default")]
    selected: AtomicBool,
}

fn atomicbool_default() -> AtomicBool {
    AtomicBool::new(false)
}

impl Hash for ActionDesc {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.description.hash(state);
        // self.command.hash(state);
    }
}

impl PartialEq for ActionDesc {
    fn eq(&self, other: &Self) -> bool {
        self.description == other.description && self.logo == other.logo
    }
}

impl Eq for ActionDesc {}

impl ActionDesc {
    fn new(
        description: SmolStr,
        command: SmolStr,
        logo: Option<u32>,
        selected: AtomicBool,
    ) -> Self {
        let args = Self::generate_args(&command);
        Self {
            timeout: 15,
            description,
            command,
            logo,
            selected,
            need_reply: true,
            arguments: args,
        }
    }

    fn generate_args(command: &str) -> ArgumentsMap {
        let args = Arc::new(
            RE.captures_iter(dbg!(command.into()))
                // .skip(1)
                .map(|caps| {
                    let c = caps.get(1).unwrap().as_str().into();
                    dbg!(&caps);
                    c
                    // dbg!(caps)

                    // caps.get(1).map(|name| {
                    //     name.as_str().into()
                    // })
                })
                .collect(),
        );
        dbg!(args)
    }
}

#[derive(Debug, Clone)]
pub enum ActionState {
    Selection,
    NeedArguments,
    Edit,
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
            ConfigError::IO(io) => write!(f, "IO Error : {}", io),
            ConfigError::Toml(toml) => write!(f, "Toml Parsing Error : {}", toml),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Config {
    clients: IndexSet<Arc<Client>>,
    actions: Vec<ActionDesc>,
    screenshot_path: SmolStr,
    // credentials: IndexSet<Cred>,
}

#[derive(Serialize, Deserialize, Hash, PartialEq, Eq)]
struct Cred {
    user: SmolStr,
    auth_method: Auth,
}

#[derive(Serialize, Deserialize, Hash, PartialEq, Eq)]
enum Auth {
    Password(SmolStr),
    PublicKey(SmolStr),
    Interactive,
    None,
}

impl Default for Cred {
    fn default() -> Self {
        Self {
            auth_method: Auth::None,
            user: SmolStr::new(""),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            clients: [].into(),
            actions: [].into(),
            screenshot_path: "/tmp/screenshot.png".into(),
            // credentials: [].into(),
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

impl Deref for MemConfig {
    type Target = Config;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for MemConfig {
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
        let mut handle = OpenOptions::new().write(true).read(true).open(path)?;

        let mut conf = String::new();
        handle.read_to_string(&mut conf)?;
        toml::from_str(&conf)
            .map_err(ConfigError::Toml)
            .map(DiskConfig)
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
        .write_all(
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
    username_input: String,
    password_input: String,

    select_actions: AtomicBool,
    edit_actions: AtomicBool,

    current_dialog: Dialog,
    dialog_inputs: Vec<Vec<String>>,
}

impl Default for Data {
    fn default() -> Self {
        Data {
            config: Default::default(),
            client_addr_input: "".into(),
            client_port_input: "22".into(),
            username_input: "".into(),
            password_input: "".into(),
            select_all: AtomicBool::new(false),

            select_actions: AtomicBool::new(false),
            edit_actions: AtomicBool::new(false),
            clients_version: AtomicUsize::new(0),
            actions_version: AtomicUsize::new(0),

            current_dialog: Dialog::Main,
            //todo
            dialog_inputs: vec![vec!["".to_owned(); 16]; 3],
        }
    }
}

impl Application for Data {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    type Theme = Theme;

    fn new(_flags: ()) -> (Data, Command<Self::Message>) {
        let diskconfig = DiskConfig::load(CONFIG_PATH.as_path()).map_or_else(
            |e| {
                if MessageDialog::new()
                    .set_level(MessageLevel::Warning)
                    .set_title(e.to_string().as_str())
                    .set_description("Do you want to load default configuration?")
                    .set_buttons(MessageButtons::YesNo)
                    .show()
                {
                    Default::default()
                } else {
                    panic!("No way of getting valid configuration")
                }
            },
            |v| v,
        );

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
            Message::CopyToClipboard(str) => {
                return clipboard::write(str.to_string());
            }
            Message::RefreshAddr => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
            }
            Message::ShowDialog(dialog) => {
                self.current_dialog = dialog;
            }
            Message::DialogInput(index, input) => {
                self.dialog_inputs[self.current_dialog.enum_index()][index] = input;
            }
            Message::Config(action) => match action {
                ConfigAction::Save => save(&self.config, None),
                ConfigAction::Export => {
                    if let Some(file) = FileDialog::new()
                        .add_filter("Configuration", &["toml"])
                        .save_file()
                    {
                        save(&self.config, Some(file.as_path()));
                    }
                }
                ConfigAction::Open => {
                    open::that(CONFIG_PATH.as_os_str()).ok();
                }
                ConfigAction::Import => {
                    if let Some(file) = FileDialog::new()
                        .add_filter("Configuration", &["toml"])
                        .pick_file()
                    {
                        match DiskConfig::load(file.as_path()) {
                            Ok(config) => {
                                self.config = config.into();
                            }
                            Err(e) => {
                                MessageDialog::new()
                                    .set_level(MessageLevel::Error)
                                    .set_title("Unable to load config")
                                    .set_description(e.to_string().as_str())
                                    .show();
                            }
                        }
                    }
                }
            },
            Message::UpdateAddrSel(state, index) => {
                if let Some(v) = self.config.clients.get_index(index) {
                    self.clients_version.fetch_add(1, Ordering::Relaxed);
                    v.selected.store(state, Ordering::Relaxed);
                }
            }
            Message::DelectAction => {
                self.actions_version.fetch_add(1, Ordering::Relaxed);
                if self.select_actions.load(Ordering::Relaxed) {
                    self.config
                        .actions
                        .retain(|a| !a.selected.load(Ordering::Relaxed));
                }
            }
            Message::AddAction(index) => {
                let inputs = &self.dialog_inputs[self.current_dialog.enum_index()];
                let arg = ActionDesc::new(
                    SmolStr::new(&inputs[1]),
                    SmolStr::new(&inputs[2]),
                    u32::from_str_radix(&inputs[0], 16).ok(),
                    atomicbool_default(),
                );
                if let Some(index) = index  && let Some(ent) = self.config.actions.get_mut(index) {
                    *ent = arg;
                } else {
                    self.config.actions.push(arg);
                }
                self.actions_version.fetch_add(1, Ordering::Relaxed);
            }
            Message::SelectAction => {
                self.select_actions.fetch_xor(true, Ordering::Relaxed);
            }
            Message::EditAction => {
                self.edit_actions.fetch_xor(true, Ordering::Relaxed);
            }
            Message::AddrInput(addr) => {
                self.client_addr_input = addr;
            }
            Message::PortInput(port) => {
                if port.parse::<u16>().is_ok() {
                    self.client_port_input = port;
                }
            }
            Message::UsernameInput(user) => {
                self.username_input = user;
            }
            Message::PasswordInput(pass) => {
                self.password_input = pass;
            }
            Message::AddrAdd => {
                if !self.client_addr_input.is_empty() && !self.username_input.is_empty() {
                    if self.config.clients.insert(Arc::new(Client {
                        address: SmolStr::from(&self.client_addr_input),
                        port: self.client_port_input.parse().unwrap_or(22),
                        user: SmolStr::from(&self.username_input),
                        pass: SmolStr::from(&self.password_input),

                        ..Default::default()
                    })) {
                        return Command::perform(async {}, |_| Message::RefreshAddr);
                    }
                }
            }

            Message::AddrDel(index) => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.config.clients.swap_remove_index(index);
            }
            Message::AddrDelAll => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.config.clients.clear();
            }
            Message::AddrDeselAll => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.config
                    .clients
                    .iter()
                    .for_each(|c| c.selected.store(false, Ordering::Relaxed));
            }
            // Message::ActionTileSelect()
            Message::Action(execute, index, decode) => {
                if let Some(a) = self.config.actions.get(index) {
                    let state = if self.select_actions.load(Ordering::Relaxed) {
                        ActionState::Selection
                    } else if self.edit_actions.load(Ordering::Relaxed) {
                        ActionState::Edit
                    } else if a.arguments.len() > 0 && !execute {
                        ActionState::NeedArguments
                    } else {
                        ActionState::None
                    };
                    match state {
                        ActionState::Edit => {
                            self.current_dialog = Dialog::AddAction(Some(index));
                            let inputs = &mut self.dialog_inputs[self.current_dialog.enum_index()];
                            inputs[0] = a.logo.map_or("".into(), |logo| format!("{:x}", logo));
                            inputs[1] = a.description.to_string();
                            inputs[2] = a.command.to_string();
                        }
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
                            let inputs = &self.dialog_inputs[self.current_dialog.enum_index()];
                            let command = SmolStr::from(if decode {
                                format(
                                    a.command.as_ref(),
                                    &a.arguments
                                        .as_ref()
                                        .into_iter()
                                        .map(|a| a.as_str())
                                        .zip(inputs.iter().map(Formattable::display))
                                        .collect::<HashMap<_, _>>(),
                                )
                                .unwrap()
                            } else {
                                a.command.deref().to_owned()
                            });

                            let need_reply = a.need_reply;
                            let action_timeout = a.timeout;
                            let clients = self
                                .config
                                .clients
                                .iter()
                                .filter(|c| {
                                    c.selected.load(Ordering::Relaxed)
                                        || self.select_all.load(Ordering::Relaxed)
                                })
                                .cloned()
                                .map(|c| {
                                    //Clone for SmolStr is cheap, as it is O(1)
                                    let command = command.clone();
                                    Command::perform(
                                        async move {
                                            let mut info = c.info.lock().await;
                                            timeout(Duration::from_secs(action_timeout), async {
                                                for _ in 0..3 {
                                                    match dbg!(
                                                        c.exec(
                                                            &mut info,
                                                            command.as_str(),
                                                            None,
                                                            need_reply
                                                        )
                                                        .await
                                                    ) {
                                                        Ok((stdout, stderr, exit_status)) => {
                                                            info.output.push_back(Log {
                                                                stdout: stdout.map(|s| {
                                                                    SmolStr::from(
                                                                        String::from_utf8_lossy(
                                                                            s.as_ref(),
                                                                        ),
                                                                    )
                                                                }),
                                                                stderr: stderr.map(|s| {
                                                                    SmolStr::from(
                                                                        String::from_utf8_lossy(
                                                                            s.as_ref(),
                                                                        ),
                                                                    )
                                                                }),
                                                                timestamp:
                                                                    chrono::offset::Local::now(),
                                                            });
                                                            info.status = if exit_status == 0 {
                                                                SUCCESSFUL
                                                            } else {
                                                                EXEC_ERR
                                                            };
                                                            break;
                                                        }
                                                        Err(e) => {
                                                            info.msg = SmolStr::from(e.to_string());
                                                            info.status = ERROR;
                                                            use russh::Error;
                                                            match e {
                                                                Error::Disconnect
                                                                | Error::SendError => {
                                                                    info.connection = None;
                                                                }
                                                                _ => {}
                                                            };
                                                        }
                                                    }
                                                }
                                            })
                                            .await
                                            .map_or_else(
                                                |_| {
                                                    info.msg = SmolStr::from("Action timed out");
                                                    info.status = TIMED_OUT;
                                                },
                                                |_| {},
                                            );
                                        },
                                        |_| Message::RefreshAddr,
                                    )
                                });

                            return Command::batch(clients);
                        }
                    }
                }
            }
            Message::Screenshot(index, buffer) => {
                if let Some(client) = self.config.clients.get_index(index) {
                    let client = client.clone();
                    let screenshot_path = self.config.screenshot_path.clone();
                    return Command::perform(
                        async move {
                            let mut screenshot = client.screenshot.lock().await;
                            if buffer && screenshot.is_some() {
                                return;
                            }
                            let mut info = client.info.lock().await;
                            if let Ok((_, Some(png))) = client
                                .download_file(&mut info, &Path::new(screenshot_path.as_str()))
                                .await
                            {
                                *screenshot = Some(Cow::Owned(png));
                            }
                        },
                        move |_| Message::ShowDialog(Dialog::Screenshot(index)),
                    );
                }
            }
            Message::None => {}
        }
        Command::none()
    }

    fn view(&self) -> Element<Self::Message> {
        let menu_button = |label: &str, msg: Message| {
            menu_tree!(button(
                text(label)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .horizontal_alignment(alignment::Horizontal::Center)
                    .vertical_alignment(alignment::Vertical::Center)
            )
            .on_press(msg)
            .width(Length::Fill)
            .padding([4, 8]))
        };
        dbg!("Update");

        let menu_column = |v, c: Vec<(&str, Message)>| {
            menu_tree(
                v,
                c.into_iter()
                    .map(|(name, msg)| menu_button(name, msg))
                    .collect(),
            )
        };
        let menu_bar = menu_bar(vec![
            menu_column(
                "Configuration",
                [
                    ("Save (Ctrl + S)", Message::Config(ConfigAction::Save)),
                    ("Import", Message::Config(ConfigAction::Import)),
                    ("Export", Message::Config(ConfigAction::Export)),
                    ("Open config folder", Message::Config(ConfigAction::Open)),
                ]
                .into(),
            ),
            menu_column(
                "File",
                [
                    ("Upload Files", Message::Config(ConfigAction::Save)),
                    ("Download Files", Message::Config(ConfigAction::Import)),
                    ("Export", Message::Config(ConfigAction::Export)),
                ]
                .into(),
            ),
        ])
        .item_width(iced_aw::ItemWidth::Static(180))
        .item_height(iced_aw::ItemHeight::Static(25))
        .spacing(4.0)
        .bounds_expand(30)
        .path_highlight(Some(iced_aw::PathHighlight::MenuActive));

        let menu_bar_style: fn(&iced::Theme) -> container::Appearance =
            |_theme| container::Appearance {
                background: Some(Color::TRANSPARENT.into()),
                ..Default::default()
            };
        let menu = container(menu_bar)
            .padding([2, 8])
            .width(Length::Fill)
            .style(menu_bar_style);

        let clients = lazy(self.clients_version.load(Ordering::Relaxed), |_| {
            scrollable(
                container(Column::with_children(
                    self.config
                        .clients
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            let (state, msg) = c
                                .info
                                .try_lock()
                                .map_or((EXECUTING, "Script is being run".into()), |l| {
                                    (l.status, l.msg.clone())
                                });
                            row![
                                checkbox(
                                    c.address.deref(),
                                    c.selected.load(Ordering::Relaxed)
                                        || self.select_all.load(Ordering::Relaxed),
                                    move |state| { Message::UpdateAddrSel(state, i) }
                                )
                                .width(Length::Fill),
                                text(c.port.to_string()).width(Length::Fill),
                                tooltip(icon(state, 40), msg, Position::Bottom),
                                button(icon(VIEW_LOG, 30))
                                    .on_press(Message::ShowDialog(Dialog::Logs(i, false)))
                                    .style(theme::Button::Text),
                                button(icon(SCREENSHOT, 30)).on_press(Message::Screenshot(i, true)),
                                button(icon(DELETE, 20))
                                    .on_press(Message::AddrDel(i))
                                    .padding(10)
                                    .style(theme::Button::Destructive),
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
                .width(Length::FillPortion(4))
                .on_input(Message::AddrInput)
                .on_submit(Message::AddrAdd),
            text_input("Port", &self.client_port_input)
                .width(Length::FillPortion(1))
                .on_input(Message::PortInput)
                .on_submit(Message::AddrAdd),
            text_input("Username", &self.username_input)
                .width(Length::FillPortion(3))
                .on_input(Message::UsernameInput)
                .on_submit(Message::AddrAdd),
            text_input("Password", &self.password_input)
                .width(Length::FillPortion(3))
                .on_input(Message::PasswordInput)
                .on_submit(Message::AddrAdd),
            button(icon(REFRESH, 20))
                .on_press(Message::RefreshAddr)
                .padding(10),
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
                self.config
                    .actions
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let logo: Element<Self::Message> =
                            if let Some(logo) = a.logo.and_then(char::from_u32) {
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
                            .on_press(Message::Action(false, i, false))
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
                .on_press(Message::ShowDialog(Dialog::AddAction(None)))
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
            button(icon(EDIT, 30))
                .padding(10)
                .style(if self.edit_actions.load(Ordering::Relaxed) {
                    theme::Button::Custom(Box::new(ActionStyleSelection))
                } else {
                    theme::Button::Secondary
                })
                .on_press(Message::EditAction)
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

        let view: Element<Message> = match self.current_dialog {
            Dialog::Main => content.into(),
            Dialog::Logs(index, show_error) => Modal::new(true, content, move || {
                let output = if let Some(client) = self.config.clients.get_index(index) && let Some(info) = client.info.try_lock() {
                    column(info.output.iter().rev().filter_map(|o| {
                        if show_error && let Some(ref output) = o.stderr {
                            Some(Card::new(row![
                                text(o.timestamp.format("%Y-%m-%d %H:%M:%S")), horizontal_space(Length::Fill),
                                button(icon(COPY, 20))
                                    .on_press(Message::CopyToClipboard(output.clone()))
                                    .padding(10),], text(output)))
                        } else if !show_error && let Some(ref output) = o.stdout {
                            Some(Card::new(row![
                                text(o.timestamp.format("%Y-%m-%d %H:%M:%S")), horizontal_space(Length::Fill),
                                button(icon(COPY, 20))
                                    .on_press(Message::CopyToClipboard(output.clone()))
                                    .padding(10),], text(output)))
                        } else {
                            None
                        }
                    }).map(Element::from).collect())
                } else {
                    column![text("Unable to load logs")]
                }.spacing(5);
                scrollable(
                    Card::new(
                        "Logs",
                        container(column![
                            container(checkbox("Show Error", show_error, move |state| Message::ShowDialog(
                                Dialog::Logs(index, state)
                            ))).width(Length::Fill).align_x(alignment::Horizontal::Left),
                            output
                        ].spacing(5))
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
            Dialog::AddAction(index) => Modal::new(true, content, move || {
                let inputs = &self.dialog_inputs[self.current_dialog.enum_index()];
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
                                    horizontal_space(Length::Fill),
                                    text_input(tip, &inputs[i]) .width(Length::Fill)
                                        .on_input(move |str| Message::DialogInput(i, str))
                                        .width(Length::Fixed(300.))
                                ]
                                .spacing(10)
                                .into()
                            })
                            .collect(),
                        )
                        .push(button("Add").on_press(Message::AddAction(index))),
                    )
                    .width(Length::Fixed(500.0))
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
                    let inputs = &self.dialog_inputs[self.current_dialog.enum_index()];
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
                                            horizontal_space(Length::Fill),
                                            text_input(arg.next().unwrap_or(""), &inputs[i],)
                                                .width(Length::Fixed(300.))
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
                                    true,
                                    index,
                                    true,
                                )))
                                .align_x(alignment::Horizontal::Right),
                            ),
                        )
                        .width(Length::Fixed(500.0))
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
            Dialog::Screenshot(index) => {
                // let png = png.to_owned();
                Modal::new(true, content, move || {
                    Card::new(
                        row![text("Screenshot"), horizontal_space(Length::Fill), button(icon(REFRESH, 20))
                                    .on_press(Message::Screenshot(index, false))
                                    .style(theme::Button::Text),
                                    ],
                        container(
                                if let Some(client) = self.config.clients.get_index(index) && let Some(png) = client.screenshot.try_lock() && let Some(ref png) = *png {
                                    Element::from(image(iced::widget::image::Handle::from_memory(png.clone())))

                                } else {
                                    text("Unable to fetch screenshot").into()
                                }
                        )
                        .padding(20)
                        .align_x(alignment::Horizontal::Center)
                        .align_y(alignment::Vertical::Center)
                        .center_x()
                        .center_y(),
                    )
                    .width(Length::Fixed(1080.0))
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

        column![menu, cont].into()
    }

    fn subscription(&self) -> iced::Subscription<Self::Message> {
        subscription::events_with(|e, s| match (e, s) {
            (
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key_code: keyboard::KeyCode::Tab,
                    modifiers,
                }),
                _,
            ) => Some(Message::Config(ConfigAction::Save)),
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
