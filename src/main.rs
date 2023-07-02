#![feature(async_closure)]
#![feature(let_chains)]
#![feature(const_for)]

use ahash::{AHasher, RandomState};
use iced::keyboard::{KeyCode, Modifiers};
use iced::{event, keyboard, subscription, Event, Subscription};
use iced_aw::date_picker::Date;
use iced_aw::time_picker::Time;
use iced_aw::Modal;
use indexmap::IndexMap;
use std::borrow::Borrow;
use std::rc::Rc;
use std::thread;
use std::time::Duration;
use tokio::time::timeout;
use tokio_retry::Action;

use ringbuf::{HeapConsumer, HeapRb, Rb};
use std::{
    default,
    fs::{self, File, OpenOptions},
    io::IoSlice,
    net::IpAddr,
    ops::Deref,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering},
        Arc,
    },
    thread::{yield_now, AccessError},
};

use serde::{Deserialize, Serialize};

use async_mutex::Mutex;
use circular_buffer::CircularBuffer;
use std::io::Read;
use std::io::Write;

use rayon::prelude::*;
use standard_paths::LocationType;

use iced::{
    alignment, color, executor,
    futures::executor::block_on,
    theme::{self, palette::Primary, Button},
    widget::{
        button::{Appearance, StyleSheet},
        checkbox, column, container, row, scrollable,
        scrollable::Properties,
        text, text_input, Checkbox, Column, Text,
    },
    Alignment, Application, Background, Color, Command, Element, Font, Length, Theme,
};
use iced_lazy::lazy;

use iced::widget::button;

use iced_aw::card::Card;
use iced_aw::style::card::CardStyles;

use russh::server::{Auth, Session};
use russh::*;

type AIndexMap<K, V> = IndexMap<K, V, AHasher>;

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

use atomic_enum::atomic_enum;
#[derive(Default, PartialEq)]
#[atomic_enum]
enum State {
    AuthFailed,
    #[default]
    Disconnected,
    Connected,
    ConnectionFailed,
    Successful,
    Executing,
    Error,
    Unknown,
}

impl Default for AtomicState {
    fn default() -> Self {
        AtomicState::new(State::Disconnected)
    }
}
impl AtomicState {
    fn icon(&self) -> char {
        match self.load(Ordering::Relaxed) {
            State::AuthFailed => '\u{e99a}',
            State::Disconnected => '\u{f239}',
            State::Connected => '\u{e157}',
            State::ConnectionFailed => '\u{e1cd}',
            State::Executing => '\u{ef64}',
            State::Successful => '\u{e877}',
            State::Unknown | State::Error => '\u{e000}',
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    ArgumentDialogDestroy,
    AddrInput(String),
    PortInput(String),
    ActionDescInput(String),
    ActionCommandInput(String),
    ActionLogoInput(String),
    ActionAdd,
    ActionArgumentDialogExecute(Arc<str>),
    AddrAdd,
    AddrDel(Addr),
    AddrDelAll,
    AddrDeselAll,
    UpdateAddrSel(bool, usize),
    Action(Arc<ActionDesc>),
    ViewLog(CircularBuffer<16384, u8>),
    CloseLog,
    ShowAddAction(bool),
    SelectAction,
    DelectAction,
    SaveConfig,
    None,
}

pub enum SEvent {
    ExecuteAction(Arc<str>),
    None,
}

#[derive(Deserialize, Clone, Serialize, Hash, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Addr {
    address: Arc<str>,
    port: u16,
}

#[derive(Default)]
struct Client {
    selected: AtomicBool,
    connection: Arc<Mutex<Connection<SshClient>>>,
    ssh: SshClient,
}

struct Connection<T: russh::client::Handler> {
    handle: Option<russh::client::Handle<T>>,
    stdout: HeapRb<u8>,
    stderr: HeapRb<u8>,
    state: char,
}

impl<T: russh::client::Handler> Default for Connection<T> {
    fn default() -> Self {
        Self {
            handle: None,
            stdout: HeapRb::new(16384),
            stderr: HeapRb::new(16384),
            state: DISCONNECTED,
        }
    }
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

#[derive(Clone, Deserialize, Serialize, Hash, Debug, PartialEq, Eq, PartialOrd)]
pub struct ActionDesc {
    description: Arc<str>,
    command: Arc<str>,
    logo: Option<char>,
    arguments: Option<Arc<[Argument]>>,
}

#[derive(Clone, Deserialize, Serialize, Hash, Debug, PartialEq, Eq, PartialOrd)]
struct Argument {
    title: Arc<str>,
    value: String,
    default_value: Arc<str>,
}

#[derive(Default)]
struct ActionEntry {
    selected: AtomicBool,
}

#[derive(Serialize, Deserialize)]
struct Config {
    clients: Vec<Addr>,
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

use lazy_static::lazy_static;

lazy_static! {
    static ref config_path: std::path::PathBuf = {
        let sp = standard_paths::default_paths!()
            .writable_location(LocationType::AppConfigLocation)
            .expect("Unable to get configuration path");
        fs::create_dir_all(&sp).expect("Unable to create inital configuration path");
        sp.join("config.toml").into()
    };
}

fn save(data: &Config) {
    // let conf = Config {
    //     clients: data.clients.into_keys().collect(),
    //     actions: data.actions.into_keys().collect(),
    //     password: data.password.clone(),
    //     user: data.user.clone(),
    // };

    File::create(config_path.as_path())
        .expect("Unable to create configuration file")
        .write(
            toml::to_string(&data)
                .expect("Unable to generate default toml configuration")
                .as_bytes(),
        )
        .expect("Unable to write to new configuration file");
}

fn execute_cmd(
    all: bool,
    clients: &IndexMap<Addr, Client>,
    command: Arc<str>,
    username: Arc<str>,
    password: Arc<str>,
    config: Arc<russh::client::Config>,
) {
    let command = Arc::new(command);
}

struct Data {
    clients: IndexMap<Addr, Client>,
    actions: IndexMap<ActionDesc, ActionEntry>,
    password: Arc<str>,
    user: Arc<str>,

    clients_version: AtomicUsize,
    actions_version: AtomicUsize,
    config: Arc<russh::client::Config>,
    select_all: AtomicBool,
    // show_log: Option<CircularBuffer<16384, u8>>,
    client_addr_input: String,
    client_port_input: String,
    select_actions: AtomicBool,
    show_add_actions: bool,
    action_command_input: String,
    action_description_input: String,
    action_logo_input: String,
    action_argument_dialog: Option<Arc<ActionDesc>>,
    action_argument_inputs: Vec<String>,

    subscription_event: SEvent,
}

#[derive(Clone, Copy, Default)]
struct SshClient;

#[async_trait::async_trait]
impl client::Handler for SshClient {
    type Error = russh::Error;
    async fn check_server_key(
        self,
        _: &russh_keys::key::PublicKey,
    ) -> Result<(Self, bool), Self::Error> {
        Ok((self, true))
    }
}

impl Default for Data {
    fn default() -> Self {
        Data {
            clients: [].into(),
            actions: [].into(),
            client_addr_input: String::new(),
            client_port_input: "22".into(),
            action_command_input: String::new(),
            action_description_input: String::new(),
            action_logo_input: String::new(),
            password: "".into(),
            user: "".into(),
            config: Arc::new(russh::client::Config {
                window_size: 16384,
                ..Default::default()
            }),
            select_all: AtomicBool::new(false),
            // show_log: None,
            select_actions: AtomicBool::new(false),
            clients_version: AtomicUsize::new(0),
            actions_version: AtomicUsize::new(0),
            show_add_actions: false,
            action_argument_dialog: None,
            action_argument_inputs: Vec::new(),

            subscription_event: SEvent::None,
        }
    }
}

impl Application for Data {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    type Theme = Theme;

    fn new(_flags: ()) -> (Data, Command<Self::Message>) {
        let Config {
            clients,
            actions,
            password,
            user,
        } = match OpenOptions::new()
            .write(true)
            .read(true)
            .open(config_path.as_path())
        {
            Ok(mut handle) => {
                let mut conf = String::new();
                handle
                    .read_to_string(&mut conf)
                    .expect("Unable to read configuration file");
                toml::from_str(&conf).expect("Unable to parse configuaration file")
            }
            Err(err) => match err.kind() {
                std::io::ErrorKind::NotFound => {
                    let def = Default::default();
                    save(&def);
                    def
                }
                _ => {
                    panic!("Unable to create/open configuration file");
                }
            },
        };

        let clients = clients
            .into_iter()
            .map(|c| (c, Default::default()))
            .collect();

        let actions = actions
            .into_iter()
            .map(|c| (c, Default::default()))
            .collect();

        (
            Data {
                clients,
                actions,
                password,
                user,
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
            Message::ArgumentDialogDestroy => {
                self.action_argument_dialog = None;
                Command::none()
            }
            Message::SaveConfig => {
                let conf = Config {
                    clients: self.clients.keys().cloned().collect(),
                    actions: self.actions.keys().cloned().collect(),
                    password: self.password.clone(),
                    user: self.user.clone(),
                };
                save(&conf);
                Command::none()
            }
            Message::UpdateAddrSel(state, index) => {
                if let Some((_, v)) = self.clients.get_index(index) {
                    self.clients_version.fetch_add(1, Ordering::Relaxed);
                    v.selected.store(state, Ordering::Relaxed);
                }
                Command::none()
            }
            Message::DelectAction => {
                self.actions_version.fetch_add(1, Ordering::Relaxed);
                if self.select_actions.load(Ordering::Relaxed) {
                    self.actions
                        .retain(|_, a| !a.selected.load(Ordering::Relaxed));
                }
                Command::none()
            }
            Message::ActionAdd => {
                self.actions.insert(
                    ActionDesc {
                        description: Arc::from(self.action_description_input.to_owned()),
                        command: Arc::from(self.action_command_input.to_owned()),
                        logo: u32::from_str_radix(&self.action_logo_input, 16)
                            .map_or_else(|_| None, |u| char::from_u32(u)),
                        arguments: None,
                    },
                    ActionEntry {
                        selected: false.into(),
                    },
                );
                self.actions_version.fetch_add(1, Ordering::Relaxed);
                Command::none()
            }
            Message::SelectAction => {
                self.actions_version.fetch_add(1, Ordering::Relaxed);
                self.select_actions.fetch_xor(true, Ordering::Relaxed);
                Command::none()
            }
            Message::ShowAddAction(show) => {
                self.show_add_actions = show;
                Command::none()
            }
            Message::ViewLog(client) => {
                // self.show_log = Some(client);
                Command::none()
            }
            Message::CloseLog => {
                // self.show_log = None;
                Command::none()
            }
            Message::AddrInput(addr) => {
                self.client_addr_input = addr;
                Command::none()
            }
            Message::PortInput(port) => {
                if port.parse::<u16>().is_ok() {
                    self.client_port_input = port;
                }
                Command::none()
            }
            Message::ActionDescInput(input) => {
                self.action_description_input = input;
                Command::none()
            }
            Message::ActionCommandInput(input) => {
                self.action_command_input = input;
                Command::none()
            }
            Message::ActionLogoInput(input) => {
                self.action_logo_input = input;
                Command::none()
            }
            Message::AddrAdd => {
                if self.client_addr_input.len() > 0 {
                    self.clients
                        .entry(Addr {
                            address: Arc::from(self.client_addr_input.clone()),
                            port: self.client_port_input.parse().unwrap_or(22),
                        })
                        .or_insert_with(|| {
                            self.clients_version.fetch_add(1, Ordering::Relaxed);
                            Client {
                                selected: AtomicBool::new(true),
                                connection: Arc::new(Mutex::new(Default::default())),
                                ssh: SshClient,
                            }
                        });
                }
                Command::none()
            }
            Message::AddrDel(client) => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.clients.remove(&client);
                Command::none()
            }
            Message::AddrDelAll => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.clients.clear();
                Command::none()
            }
            Message::AddrDeselAll => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.clients
                    .iter()
                    .for_each(|(_, c)| c.selected.store(false, Ordering::Relaxed));
                Command::none()
            }
            Message::ActionArgumentDialogExecute(command) => {
                // execute_cmd(
                //     self.select_all.load(Ordering::Relaxed),
                //     &self.clients,
                //     command,
                //     self.user.clone(),
                //     self.password.clone(),
                //     self.config.clone(),
                // );
                Command::none()
            }
            // Message::ActionTileSelect()
            Message::Action(actiondesc) => {
                // if let Some(action) = self.actions.get(actiondesc.deref()) {
                //     if self.select_actions.load(Ordering::Relaxed) {
                //         self.actions_version.fetch_add(1, Ordering::Relaxed);
                //         action.selected.fetch_xor(true, Ordering::Relaxed);
                //     } else {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                // if actiondesc.arguments.is_some() {
                //     self.action_argument_dialog = Some(actiondesc);
                // } else {
                let config = self.config.clone();
                let user = self.user.clone();
                let password = self.password.clone();
                let command = actiondesc.command.clone();
                let clients: Vec<_> = self
                    .clients
                    .iter()
                    .filter(|(_, c)| {
                        c.selected.load(Ordering::Relaxed)
                            || self.select_all.load(Ordering::Relaxed)
                    })
                    .map(|(a, c)| (a.address.clone(), a.port, c.connection.clone()))
                    .collect();

                thread::scope(|s| {
                    for (addr, port, session) in &clients {
                        s.spawn(async || {
                            timeout(Duration::from_secs(5), async {
                                if let Some(mut session) = session.try_lock_arc() {
                                    let res: Result<_, russh::Error> = async {
                                        if session.handle.is_none() {
                                            let mut conn = russh::client::connect(
                                                config.clone(),
                                                (addr.deref(), port.clone()),
                                                SshClient {},
                                            )
                                            .await?;
                                            if conn
                                                .authenticate_password(
                                                    user.deref(),
                                                    password.deref(),
                                                )
                                                .await?
                                            {
                                                session.handle = Some(conn);
                                            } else {
                                                todo!();
                                            }
                                        }
                                        if let Some(conn) = session.handle.as_mut() {
                                            let mut sess = conn.channel_open_session().await?;

                                            sess.exec(true, command.as_bytes()).await?;
                                            while let Some(msg) = sess.wait().await {
                                                match msg {
                                                    russh::ChannelMsg::ExtendedData {
                                                        ref data,
                                                        ..
                                                    } => {
                                                        session
                                                            .stderr
                                                            .push_slice_overwrite(data.deref());
                                                    }
                                                    russh::ChannelMsg::Data { ref data } => {
                                                        session
                                                            .stdout
                                                            .push_slice_overwrite(data.deref());
                                                    }
                                                    russh::ChannelMsg::ExitStatus {
                                                        exit_status,
                                                    } => {
                                                        if exit_status == 0 {
                                                            return Ok(SUCCESSFUL);
                                                        } else {
                                                            return Ok(ERROR);
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            sess.close().await?;
                                        }

                                        Ok(UNKNOWN)
                                    }
                                    .await;

                                    session.state = match res {
                                        Ok(state) => state,
                                        Err(err) => match err {
                                            russh::Error::NotAuthenticated => AUTH_FAILED,
                                            russh::Error::Disconnect | russh::Error::HUP => {
                                                DISCONNECTED
                                            }
                                            _ => UNKNOWN,
                                        },
                                    }
                                }
                            })
                            .await
                            .ok();
                        });
                    }
                });

                Command::none()
            }
            Message::None => Command::none(),
        }
    }

    fn view(&self) -> Element<Self::Message> {
        let clients = lazy(self.clients_version.load(Ordering::Relaxed), |_| {
            scrollable(
                container(Column::with_children(
                    self.clients
                        .iter()
                        .enumerate()
                        .map(|(i, (a, c))| {
                            row![
                                checkbox(
                                    a.address.deref(),
                                    c.selected.load(Ordering::Relaxed)
                                        || self.select_all.load(Ordering::Relaxed),
                                    move |state| { Message::UpdateAddrSel(state, i) }
                                )
                                .width(Length::Fill),
                                text(a.port.to_string()).width(Length::Fill),
                                icon(
                                    c.connection.try_lock().map(|c| c.state).unwrap_or(UNKNOWN),
                                    40
                                ),
                                // button(icon(VIEW_LOG, 30))
                                //     // .on_press(Message::ViewLog(b.stdout))
                                //     .style(theme::Button::Text),
                                // button(icon(VIEW_ERROR_LOG, 30))
                                //     // .on_press(Message::ViewLog(b.stderr))
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
                self.actions
                    .iter()
                    .map(|(ad, a)| {
                        let logo: Element<Self::Message> = if let Some(logo) = ad.logo {
                            icon(logo, 50).into()
                        } else {
                            row![].into()
                        };
                        container(
                            button(
                                container(
                                    column![
                                        logo,
                                        text(ad.description.deref())
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
                            .on_press(Message::Action(Arc::new(ad.clone())))
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

        // let card: Element<Self::Message> = if let Some((ref client, err)) = self.show_log {
        //     let (front, back) = if err {
        //         client.stderr.as_slices()
        //     } else {
        //         client.stdout.as_slices()
        //     };
        //     Card::new(
        //         text(if err { "Error Log" } else { "Log" }),
        //         text(String::from_utf8_lossy([front, back].concat().as_ref())),
        //     )
        //     .style(CardStyles::Info)
        //     .on_close(Message::CloseLog)
        //     .into()
        // } else {
        //     row![].into()
        // };

        let mut add = vec![
            button(icon(ADD_ACTION, 30))
                .padding(10)
                .style(theme::Button::Secondary)
                .on_press(Message::ShowAddAction(true))
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
        let cont: Element<Self::Message> = Modal::new(self.show_add_actions, content, || {
            Card::new(
                "New Action",
                container(column![
                    text_input("Icon", &self.action_logo_input)
                        .width(Length::Fill)
                        .on_input(Message::ActionLogoInput)
                        .on_submit(Message::ActionAdd),
                    text_input("Description", &self.action_description_input)
                        .width(Length::Fill)
                        .on_input(Message::ActionDescInput)
                        .on_submit(Message::ActionAdd),
                    text_input("Command", &self.action_command_input)
                        .width(Length::Fill)
                        .on_input(Message::ActionCommandInput)
                        .on_submit(Message::ActionAdd),
                ])
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
        .on_esc(Message::ShowAddAction(false))
        .into();

        // let test: Element<Self::Message> =
        //     Modal::new(self.action_argument_dialog.is_some(), content, || {
        //         Card::new(
        //             "Action Arguments",
        //             container(Column::with_children(
        //                 self.action_argument_dialog
        //                     .as_ref()
        //                     .unwrap()
        //                     .arguments
        //                     .unwrap()
        //                     .iter()
        //                     .enumerate()
        //                     .map(|(i, a)| {
        //                         text_input(
        //                             &a.title,
        //                             self.action_argument_inputs.get(i).unwrap_or(&"".to_owned()),
        //                         )
        //                         .width(Length::Fill)
        //                         .on_input(|str| {
        //                             if let Some(input) = self.action_argument_inputs.get_mut(i) {
        //                                 std::mem::replace(input, str);
        //                             }
        //                             Message::None
        //                         })
        //                     })
        //                     .map(Element::from)
        //                     .collect(),
        //             ))
        //             .width(Length::Fixed(300.0))
        //             .padding(20)
        //             .align_x(alignment::Horizontal::Center)
        //             .align_y(alignment::Vertical::Center)
        //             .center_x()
        //             .center_y(),
        //         )
        //         .width(Length::Shrink)
        //         .height(Length::Shrink)
        //         .style(CardStyles::Secondary)
        //         .into()
        //     })
        //     .on_esc(Message::ArgumentDialogDestroy)
        //     .into();

        let view = scrollable(
            container(column![cont,])
                .width(Length::Fill)
                .padding(40)
                .align_x(iced::alignment::Horizontal::Center)
                .center_x(),
        );
        view.into()
    }

    fn subscription(&self) -> iced::Subscription<Self::Message> {
        match self.subscription_event {
            SEvent::ExecuteAction(ref action) => {
                // self.subscription_event = SEvent::None;
                // subscription::unfold(, , )
                Subscription::none()
            }
            SEvent::None => Subscription::none(),
        }
        // subscription::events_with(|e, s| match (e, s) {
        //     (
        //         Event::Keyboard(keyboard::Event::KeyPressed {
        //             key_code: keyboard::KeyCode::S,
        //             modifiers: Modifiers::CTRL,
        //         }),
        //         _,
        //     ) => Some(Message::SaveConfig),

        //     _ => None,
        // })
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
