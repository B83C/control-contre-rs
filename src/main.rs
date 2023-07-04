#![feature(async_closure)]
#![feature(let_chains)]
#![feature(const_for)]

use ahash::AHasher;
use interpolator::{format, Formattable};

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

use indexmap::IndexMap;

use std::{collections::HashMap, time::Duration};
use std::{iter::once, thread};
use tokio::time::timeout;
use tokio_retry::Action;

use ringbuf::{HeapRb, Rb};
use std::{
    fs::{self, File, OpenOptions},
    ops::Deref,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
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
use russh::*;

type AIndexMap<K, V> = IndexMap<K, V, AHasher>;

use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref RE: Regex = Regex::new(r"\{(.*\#)?(\w*)(.*)\}").unwrap();
}

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

#[derive(Debug, Clone)]
pub enum Message {
    ShowDialog(Dialog),
    DialogInput(usize, String),
    AddrInput(String),
    PortInput(String),
    ActionAdd,
    AddrAdd,
    AddrDel(Addr),
    AddrDelAll,
    AddrDeselAll,
    UpdateAddrSel(bool, usize),
    Action(ActionState, usize, bool),
    SelectAction,
    DelectAction,
    SaveConfig,
    None,
}

#[derive(Debug, Clone)]
pub enum Dialog {
    Main,
    AddAction,
    ArgumentsForAction(usize, Arc<Vec<Argument>>),
    Logs(usize, bool),
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
    logo: Option<Arc<str>>,
    #[serde(skip)]
    logo_de: Option<char>,
    #[serde(skip)]
    command_de: Option<Arc<str>>,
    #[serde(skip)]
    arguments: Arc<Vec<Argument>>,
}

#[derive(Clone, Deserialize, Serialize, Hash, Debug, PartialEq, Eq, PartialOrd)]
pub struct Argument {
    name: Arc<str>,
    default_value: Arc<str>,
}

#[derive(Default)]
struct ActionEntry {
    selected: AtomicBool,
}

#[derive(Debug, Clone)]
pub enum ActionState {
    Selection,
    NeedArguments,
    None,
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

lazy_static! {
    static ref config_path: std::path::PathBuf = {
        let sp = standard_paths::default_paths!()
            .writable_location(LocationType::AppConfigLocation)
            .expect("Unable to get configuration path");
        fs::create_dir_all(&sp).expect("Unable to create inital configuration path");
        sp.join("config.toml")
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

    current_dialog: Dialog,
    dialog_inputs: Vec<String>,
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
            .map(|mut c| {
                let args = RE
                    .captures_iter(c.command.deref())
                    .map(|caps| Argument {
                        default_value: (caps.get(1).map(|s| s.as_str()).unwrap_or("")).into(),
                        name: (caps.get(2).map(|s| s.as_str()).unwrap_or_else(|| todo!())).into(),
                    })
                    .collect();
                c.arguments = Arc::new(args);
                c.command_de = dbg!(Some(Arc::from(RE.replace_all(c.command.deref(), "{$2$3}"))));
                (c, Default::default())
            })
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
            Message::ShowDialog(dialog) => {
                self.current_dialog = dialog;
            }
            Message::DialogInput(index, input) => {
                self.dialog_inputs[index] = input;
            }
            Message::SaveConfig => {
                let conf = Config {
                    clients: self.clients.keys().cloned().collect(),
                    actions: self.actions.keys().cloned().collect(),
                    password: self.password.clone(),
                    user: self.user.clone(),
                };
                save(&conf);
            }
            Message::UpdateAddrSel(state, index) => {
                if let Some((_, v)) = self.clients.get_index(index) {
                    self.clients_version.fetch_add(1, Ordering::Relaxed);
                    v.selected.store(state, Ordering::Relaxed);
                }
            }
            Message::DelectAction => {
                self.actions_version.fetch_add(1, Ordering::Relaxed);
                if self.select_actions.load(Ordering::Relaxed) {
                    self.actions
                        .retain(|_, a| !a.selected.load(Ordering::Relaxed));
                }
            }
            Message::ActionAdd => {
                let command: Arc<str> = Arc::from(self.dialog_inputs[2].as_str());
                let command_de: Arc<str> = Arc::from(RE.replace_all(command.deref(), "{$2$3}"));
                let args = RE
                    .captures_iter(command.deref())
                    .map(|caps| Argument {
                        default_value: (caps.get(1).map(|s| s.as_str()).unwrap_or("")).into(),
                        name: (caps.get(2).map(|s| s.as_str()).unwrap_or_else(|| todo!())).into(),
                    })
                    .collect();
                self.actions.insert(
                    ActionDesc {
                        description: Arc::from(self.dialog_inputs[1].as_str()),
                        command: command.clone(),
                        logo: if self.dialog_inputs[0].trim().is_empty() {
                            None
                        } else {
                            Some(Arc::from(self.dialog_inputs[0].as_str()))
                        },
                        logo_de: u32::from_str_radix(&self.dialog_inputs[0], 16)
                            .map_or_else(|_| None, char::from_u32),
                        command_de: Some(command_de),
                        arguments: Arc::new(dbg!(args)),
                    },
                    ActionEntry {
                        selected: false.into(),
                    },
                );
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
                            }
                        });
                }
            }
            Message::AddrDel(client) => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.clients.remove(&client);
            }
            Message::AddrDelAll => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.clients.clear();
            }
            Message::AddrDeselAll => {
                self.clients_version.fetch_add(1, Ordering::Relaxed);
                self.clients
                    .iter()
                    .for_each(|(_, c)| c.selected.store(false, Ordering::Relaxed));
            }
            // Message::ActionTileSelect()
            Message::Action(state, index, decode) => {
                if let Some((ad, a)) = self.actions.get_index(index) {
                    match state {
                        ActionState::Selection => {
                            self.actions_version.fetch_add(1, Ordering::Relaxed);
                            a.selected.fetch_xor(true, Ordering::Relaxed);
                        }
                        ActionState::NeedArguments => {
                            self.current_dialog =
                                Dialog::ArgumentsForAction(index, ad.arguments.clone());
                        }
                        ActionState::None => {
                            self.clients_version.fetch_add(1, Ordering::Relaxed);
                            let config = self.config.clone();
                            let user = self.user.clone();
                            let password = self.password.clone();
                            let command = if decode {
                                format(
                                    ad.command_de.as_ref().unwrap().deref(),
                                    &ad.arguments
                                        .as_ref()
                                        .into_iter()
                                        .zip(self.dialog_inputs.iter())
                                        .map(|(a, b)| (a.name.as_ref(), Formattable::display(b)))
                                        .collect::<HashMap<_, _>>(),
                                )
                                .unwrap()
                            } else {
                                ad.command.deref().to_owned()
                            };
                            let clients: Vec<_> = self
                                .clients
                                .iter()
                                .filter(|(_, c)| {
                                    c.selected.load(Ordering::Relaxed)
                                        || self.select_all.load(Ordering::Relaxed)
                                })
                                .map(|(a, c)| (a.address.clone(), a.port, c.connection.clone()))
                                .collect();

                            return Command::perform(
                                async move {
                                    tokio_scoped::scope(|s| {
                                        for (addr, port, session) in &clients {
                                            s.spawn(async {
                                                timeout(Duration::from_secs(5), async {
                                                    if let Some(mut session) = session.try_lock() {
                                                        let res: Result<_, russh::Error> = async {
                                                            if session.handle.is_none() {
                                                                let mut conn =
                                                                    russh::client::connect(
                                                                        config.clone(),
                                                                        (addr.deref(), *port),
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
                                                            if let Some(conn) =
                                                                session.handle.as_mut()
                                                            {
                                                                let mut sess = conn
                                                                    .channel_open_session()
                                                                    .await?;

                                                                sess.exec(true, command.as_bytes())
                                                                    .await?;
                                                                while let Some(msg) =
                                                                    sess.wait().await
                                                                {
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

                                                        session.state = match dbg!(res) {
                                                            Ok(state) => state,
                                                            Err(err) => match err {
                                                                russh::Error::NotAuthenticated => {
                                                                    AUTH_FAILED
                                                                }
                                                                russh::Error::Disconnect
                                                                | russh::Error::HUP => DISCONNECTED,
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
        dbg!("Update");

        // let menu = menu_bar!(menu_tree("test", vec![menu_tree!("he")]));
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
                                button(icon(VIEW_LOG, 30))
                                    .on_press(Message::ShowDialog(Dialog::Logs(i, false)))
                                    .style(theme::Button::Text),
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
                self.actions
                    .iter()
                    .enumerate()
                    .map(|(i, (ad, a))| {
                        let logo: Element<Self::Message> = if let Some(logo) = ad.logo_de {
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
                            .on_press(Message::Action(
                                if self.select_actions.load(Ordering::Relaxed) {
                                    ActionState::Selection
                                } else if ad.arguments.len() > 0 {
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
                let output = if let Some((_, client)) = self.clients.get_index(index) {
                    if let Some(session) = client.connection.try_lock() {
                        let (head, end) = if show_error {
                            session.stderr.as_slices()
                        } else {
                            session.stdout.as_slices()
                        };
                        String::from_utf8([head, end].concat())
                            .unwrap_or("Unable to concat logs".into())
                    } else {
                        "Still Executing".into()
                    }
                } else {
                    "Invalid client".into()
                };
                scrollable(
                    Card::new(
                        "Logs",
                        container(column![
                            checkbox("Show Error", show_error, move |state| Message::ShowDialog(
                                Dialog::Logs(index, state)
                            )),
                            text(output).width(Length::Fill)
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
                                    .map(|(i, a)| {
                                        row![
                                            text(&a.name),
                                            text_input(&a.default_value, &self.dialog_inputs[i],)
                                                .width(Length::Fill)
                                                .on_input(move |str| Message::DialogInput(i, str))
                                        ]
                                        .spacing(20)
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

        scrollable(
            container(view)
                .width(Length::Fill)
                .padding(40)
                .align_x(iced::alignment::Horizontal::Center)
                .center_x(),
        )
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
            ) => Some(Message::SaveConfig),

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
