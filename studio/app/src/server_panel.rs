//! The PSF Guard panel: connecting Studio to servers, and opening a target's
//! channel stacks from one.
//!
//! **PSF Guard** in the top bar opens the panel over the canvas. The left
//! column lists the servers Studio is connected to and has the address field
//! to connect to another. Choosing a server lists its databases, then a
//! database's projects, then a project's targets with their channel stacks.
//! **Open** prepares the chosen target, as a link from PSF Guard does.
//!
//! Connecting to a server with sign-in shows a code and opens the server's
//! **Connect PSF Studio** page in the browser. Studio asks the server every
//! few seconds whether the code was allowed, and keeps the token it then
//! gets in the Keychain.

use super::panes::{CONTROL, PANE, QUIET, SELECTED, TEXT};
use super::server_documents::{Fetching, Which};
use super::servers::{
    self, computer_name, forget_token, keep_token, kept_token, normal_address, Client, Database,
    DocumentSummary, Failure, Project, ServerList, ServerRecord, StackedTarget, Target,
};
use super::Studio;
use gpui::{
    div, prelude::*, px, rgb, rgba, ClipboardItem, Context, FocusHandle, KeyDownEvent,
    SharedString, Window,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Something fetched on a thread of its own.
pub(super) enum Load<T> {
    Idle,
    Loading(Arc<Mutex<Option<Result<T, Failure>>>>),
    Done(Result<T, Failure>),
}

impl<T: Send + 'static> Load<T> {
    fn start(fetch: impl FnOnce() -> Result<T, Failure> + Send + 'static) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let filled = slot.clone();
        std::thread::spawn(move || {
            let result = fetch();
            *filled.lock().expect("load") = Some(result);
        });
        Self::Loading(slot)
    }

    /// Takes in a finished fetch. Whether it finished now.
    fn follow(&mut self) -> bool {
        let Self::Loading(slot) = self else {
            return false;
        };
        let Some(result) = slot.lock().expect("load").take() else {
            return false;
        };
        *self = Self::Done(result);
        true
    }

    fn done(&self) -> Option<&T> {
        match self {
            Self::Done(Ok(value)) => Some(value),
            _ => None,
        }
    }

    fn failure(&self) -> Option<&Failure> {
        match self {
            Self::Done(Err(failure)) => Some(failure),
            _ => None,
        }
    }
}

/// Where a connect stands.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Connecting {
    Checking,
    /// Waiting for a person to allow the code in the browser.
    Waiting {
        code: String,
        approve_url: String,
    },
    Connected(ServerRecord),
    Failed(String),
}

/// A connect under way, on a thread of its own.
pub(super) struct ConnectRun {
    state: Arc<Mutex<Connecting>>,
    cancel: Arc<AtomicBool>,
}

impl ConnectRun {
    fn start(address: String) -> Self {
        let state = Arc::new(Mutex::new(Connecting::Checking));
        let cancel = Arc::new(AtomicBool::new(false));
        let run = Self {
            state: state.clone(),
            cancel: cancel.clone(),
        };
        std::thread::spawn(move || {
            let outcome = connect(&address, &state, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                *state.lock().expect("connect") = match outcome {
                    Ok(record) => Connecting::Connected(record),
                    Err(reason) => Connecting::Failed(reason),
                };
            }
        });
        run
    }

    fn state(&self) -> Connecting {
        self.state.lock().expect("connect").clone()
    }
}

/// Connects to `address`: at once when it has no sign-in, otherwise with a
/// code a person allows in the browser.
fn connect(
    address: &str,
    state: &Mutex<Connecting>,
    cancel: &AtomicBool,
) -> Result<ServerRecord, String> {
    let client = Client::new(address, None);
    let status = client
        .auth_status()
        .map_err(|failure| failure.to_string())?;
    if !status.authentication_required {
        return Ok(ServerRecord {
            address: address.to_string(),
            signs_in: false,
            username: None,
            saves_documents: true,
            expires_at: None,
        });
    }
    let started = client
        .start_connect(&computer_name())
        .map_err(|failure| failure.to_string())?;
    let approve_url = format!("{address}{}", started.approve_path);
    *state.lock().expect("connect") = Connecting::Waiting {
        code: started.user_code.clone(),
        approve_url: approve_url.clone(),
    };
    open_in_browser(&approve_url);
    let pause = Duration::from_secs(started.poll_seconds.clamp(1, 10));
    loop {
        // Wait in small steps, so Cancel is quick.
        for _ in 0..(pause.as_millis() / 100) {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".to_string());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let answer = client
            .trade_connect(&started.connect_id)
            .map_err(|failure| failure.to_string())?;
        match answer.status.as_str() {
            "waiting" => continue,
            "denied" => return Err("The connection was denied in the browser".to_string()),
            "expired" => {
                return Err(
                    "The code expired before it was allowed. Connect again for a new code."
                        .to_string(),
                )
            }
            "approved" => {
                let token = answer
                    .token
                    .ok_or_else(|| "The server approved the code but sent no token".to_string())?;
                keep_token(address, &token)?;
                return Ok(ServerRecord {
                    address: address.to_string(),
                    signs_in: true,
                    username: answer.username,
                    saves_documents: answer.saves_documents.unwrap_or(false),
                    expires_at: answer.expires_at,
                });
            }
            other => return Err(format!("The server answered {other}")),
        }
    }
}

fn open_in_browser(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// A link's target, waiting for the person to accept the account its ticket
/// connected as.
pub(super) struct LinkConnect {
    pub record: ServerRecord,
    pub token: String,
    pub target: super::open::TargetLink,
}

/// The panel's state.
pub(super) struct ServerPanel {
    pub open: bool,
    list: ServerList,
    address: String,
    address_focus: Option<FocusHandle>,
    connecting: Option<ConnectRun>,
    message: Option<String>,
    server: Option<String>,
    databases: Load<Vec<Database>>,
    database: Option<String>,
    projects: Load<Vec<Project>>,
    project: Option<i32>,
    targets: Load<(Vec<Target>, Vec<StackedTarget>)>,
    target: Option<i32>,
    /// The chosen target's Studio documents on the server.
    documents: Load<Vec<DocumentSummary>>,
    document: Which,
    /// A document being fetched before its picture is prepared, with the
    /// token the preparing needs.
    pub fetching: Option<(super::open::TargetLink, Option<String>, Fetching, bool)>,
    /// A document whose stacks were registered anew, waiting for the person
    /// to open it with the new stacks or not.
    pub reframed: Option<(super::open::TargetLink, Which, bool)>,
    /// A link's ticket traded for a token, waiting for the person.
    pub link: Option<LinkConnect>,
    /// A link's ticket being traded.
    pub trading: Option<(super::open::TargetLink, Load<servers::TokenAnswer>)>,
}

impl Default for ServerPanel {
    fn default() -> Self {
        Self {
            open: false,
            list: ServerList::load(&ServerList::path()),
            address: String::new(),
            address_focus: None,
            connecting: None,
            message: None,
            server: None,
            databases: Load::Idle,
            database: None,
            projects: Load::Idle,
            project: None,
            targets: Load::Idle,
            target: None,
            documents: Load::Idle,
            document: Which::Latest,
            fetching: None,
            reframed: None,
            link: None,
            trading: None,
        }
    }
}

impl ServerPanel {
    /// A client for a server in the list, with its kept token.
    pub(super) fn client(&self, address: &str) -> Option<Client> {
        let record = self.list.find(address)?;
        let token = if record.signs_in {
            Some(kept_token(address)?)
        } else {
            None
        };
        Some(Client::new(address, token))
    }

    pub(super) fn record(&self, address: &str) -> Option<ServerRecord> {
        self.list.find(address).cloned()
    }

    fn save_list(&mut self) {
        if let Err(error) = self.list.save(&ServerList::path()) {
            self.message = Some(format!("The server list was not saved: {error}"));
        }
    }

    pub(super) fn keep_server(&mut self, record: ServerRecord) {
        self.list.keep(record);
        self.save_list();
    }

    /// A failure as the panel words it, and what it changes: a server that
    /// signed Studio out is shown signed out.
    fn take_failure(&mut self, failure: &Failure) -> String {
        if *failure == Failure::SignedOut
            && let Some(address) = self.server.clone()
        {
            forget_token(&address);
            return "Studio is signed out of this server: its connection was revoked or has \
                    expired. Choose Connect again."
                .to_string();
        }
        failure.to_string()
    }
}

impl Studio {
    pub(super) fn toggle_server_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.servers.open = !self.servers.open;
        if self.servers.open {
            let focus = self
                .servers
                .address_focus
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            if self.servers.list.servers.is_empty() {
                focus.focus(window, cx);
            }
        }
    }

    /// Takes in what threads fetched or answered.
    pub(super) fn follow_servers(&mut self) {
        let panel = &mut self.servers;
        if let Some(run) = &panel.connecting {
            match run.state() {
                Connecting::Connected(record) => {
                    let address = record.address.clone();
                    panel.connecting = None;
                    panel.address.clear();
                    panel.message = Some(match &record.username {
                        Some(username) if record.saves_documents => {
                            format!("Connected to {address} as {username}.")
                        }
                        Some(username) => format!(
                            "Connected to {address} as {username}. The account is read-only, so \
                             documents are saved on this computer only."
                        ),
                        None => format!("Connected to {address}, which has no sign-in."),
                    });
                    panel.keep_server(record);
                    self.choose_server(address);
                    return;
                }
                Connecting::Failed(reason) => {
                    panel.connecting = None;
                    panel.message = Some(reason);
                }
                _ => {}
            }
        }
        let mut failures = Vec::new();
        if panel.databases.follow()
            && let Some(failure) = panel.databases.failure()
        {
            failures.push(failure.clone());
        }
        if panel.projects.follow()
            && let Some(failure) = panel.projects.failure()
        {
            failures.push(failure.clone());
        }
        if panel.targets.follow()
            && let Some(failure) = panel.targets.failure()
        {
            failures.push(failure.clone());
        }
        if panel.documents.follow()
            && let Some(failure) = panel.documents.failure()
        {
            failures.push(failure.clone());
        }
        if let Some(failure) = failures.first() {
            let message = panel.take_failure(failure);
            panel.message = Some(message);
        }
        self.follow_link_ticket();
        self.follow_fetching();
    }

    fn choose_server(&mut self, address: String) {
        let panel = &mut self.servers;
        panel.database = None;
        panel.project = None;
        panel.target = None;
        panel.projects = Load::Idle;
        panel.targets = Load::Idle;
        panel.databases = match panel.client(&address) {
            Some(client) => Load::start(move || client.databases()),
            None => Load::Done(Err(Failure::SignedOut)),
        };
        panel.server = Some(address);
    }

    fn choose_database(&mut self, database: String) {
        let panel = &mut self.servers;
        let Some(client) = panel
            .server
            .as_deref()
            .and_then(|server| panel.client(server))
        else {
            return;
        };
        panel.project = None;
        panel.target = None;
        panel.targets = Load::Idle;
        let id = database.clone();
        panel.projects = Load::start(move || client.projects(&id));
        panel.database = Some(database);
    }

    fn choose_project(&mut self, project: i32) {
        let panel = &mut self.servers;
        let (Some(client), Some(database)) = (
            panel
                .server
                .as_deref()
                .and_then(|server| panel.client(server)),
            panel.database.clone(),
        ) else {
            return;
        };
        panel.target = None;
        panel.targets = Load::start(move || {
            let targets = client.targets(&database, project)?;
            // A project without color stacks has no list; its targets show
            // without stacks.
            let stacked = client
                .stacked_targets(&database, project)
                .unwrap_or_default();
            Ok((targets, stacked))
        });
        panel.project = Some(project);
    }

    fn choose_target(&mut self, target: i32) {
        let panel = &mut self.servers;
        let (Some(client), Some(database), Some(project)) = (
            panel
                .server
                .as_deref()
                .and_then(|server| panel.client(server)),
            panel.database.clone(),
            panel.project,
        ) else {
            return;
        };
        panel.target = Some(target);
        panel.document = Which::Latest;
        panel.documents = Load::start(move || client.documents(&database, project, target));
    }

    /// Lists a target's documents again when the panel shows that target:
    /// a save may have added one.
    pub(super) fn refresh_documents_of(&mut self, (server, project, target): &(String, i32, i32)) {
        let panel = &self.servers;
        if panel.server.as_deref() == Some(server.as_str())
            && panel.project == Some(*project)
            && panel.target == Some(*target)
        {
            let which = panel.document.clone();
            self.choose_target(*target);
            self.servers.document = which;
        }
    }

    /// Resolves a conflict, as **Save as a copy** or **Open the newer
    /// version** does.
    pub(super) fn control_resolve_conflict(
        &mut self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        if !self.server_save.conflict() {
            return Err("The picture has no conflict".to_string());
        }
        match params["keep"].as_str() {
            Some("copy") => self.save_as_copy(),
            Some("newer") => self.open_newer_version(),
            _ => return Err("`keep` is \"copy\" or \"newer\"".to_string()),
        }
        Ok(self.control_servers())
    }

    fn start_connect(&mut self) {
        let panel = &mut self.servers;
        match normal_address(&panel.address) {
            Ok(address) => {
                panel.message = servers::sends_in_the_clear(&address).then(|| {
                    format!(
                        "{address} uses plain HTTP: the token travels unencrypted. Use https:// \
                         when the server is on another network."
                    )
                });
                panel.connecting = Some(ConnectRun::start(address));
            }
            Err(reason) => panel.message = Some(reason),
        }
    }

    fn cancel_connect(&mut self) {
        if let Some(run) = self.servers.connecting.take() {
            run.cancel.store(true, Ordering::Relaxed);
        }
        self.servers.message = None;
    }

    /// Signs out of a server: its token is revoked on the server and deleted
    /// from the Keychain, and it leaves the list.
    fn sign_out(&mut self, address: String) {
        if let Some(client) = self.servers.client(&address) {
            // The server may be out of reach; the token goes from here
            // either way, and it expires on its own.
            std::thread::spawn(move || {
                let _ = client.disconnect();
            });
        }
        forget_token(&address);
        self.servers.list.remove(&address);
        self.servers.save_list();
        if self.servers.server.as_deref() == Some(address.as_str()) {
            self.servers.server = None;
            self.servers.databases = Load::Idle;
            self.servers.projects = Load::Idle;
            self.servers.targets = Load::Idle;
        }
        self.servers.message = Some(format!("Signed out of {address}."));
    }

    /// Opens the chosen target's channel stacks.
    fn open_chosen_target(&mut self) {
        let panel = &self.servers;
        let (Some(server), Some(database), Some(project), Some(target)) = (
            panel.server.clone(),
            panel.database.clone(),
            panel.project,
            panel.target,
        ) else {
            return;
        };
        let which = panel.document.clone();
        self.servers.open = false;
        self.open_server_document(
            super::open::TargetLink {
                server,
                database,
                project,
                target,
                ticket: None,
                document: None,
            },
            which,
            false,
        );
    }

    /// The chosen target, as a link names it.
    fn chosen_target(&self) -> Option<super::open::TargetLink> {
        let panel = &self.servers;
        Some(super::open::TargetLink {
            server: panel.server.clone()?,
            database: panel.database.clone()?,
            project: panel.project?,
            target: panel.target?,
            ticket: None,
            document: None,
        })
    }

    /// Puts the open picture on the chosen target.
    fn save_picture_to_chosen(&mut self) {
        let Some(target) = self.chosen_target() else {
            return;
        };
        match self.save_picture_to(&target) {
            Ok(()) => {
                self.servers.message =
                    Some("The picture is saved to the target as a new document.".to_string());
                self.servers.open = false;
            }
            Err(reason) => self.servers.message = Some(reason),
        }
    }

    /// A link to a server Studio is connected to opens with that
    /// connection; a link that carries a ticket connects first.
    pub(super) fn open_link(&mut self, link: super::open::TargetLink) {
        let known = self.servers.list.find(&link.server).cloned();
        match (known, &link.ticket) {
            (Some(record), _) => {
                let token = if record.signs_in {
                    kept_token(&record.address)
                } else {
                    None
                };
                if record.signs_in && token.is_none() {
                    self.servers.open = true;
                    self.servers.message = Some(format!(
                        "Studio is signed out of {}. Connect again, then open the target.",
                        record.address
                    ));
                    self.servers.address = record.address;
                    return;
                }
                let which = link.document.clone().map_or(Which::Latest, Which::Document);
                self.open_server_document(link, which, false);
            }
            (None, Some(ticket)) => {
                let client = Client::new(&link.server, None);
                let ticket = ticket.clone();
                let name = computer_name();
                let trade = Load::start(move || client.trade_ticket(&ticket, &name));
                self.servers.trading = Some((link, trade));
                self.say("Connecting to the server the link came from…");
            }
            // A server without sign-in needs no ticket: Studio connects
            // to it as it opens the target.
            (None, None) => {
                self.servers.keep_server(ServerRecord {
                    address: link.server.clone(),
                    signs_in: false,
                    username: None,
                    saves_documents: true,
                    expires_at: None,
                });
                let which = link.document.clone().map_or(Which::Latest, Which::Document);
                self.open_server_document(link, which, false);
            }
        }
    }

    fn follow_link_ticket(&mut self) {
        let Some((_, trade)) = &mut self.servers.trading else {
            return;
        };
        if !trade.follow() {
            return;
        }
        let Some((link, Load::Done(answer))) = self.servers.trading.take() else {
            return;
        };
        match answer {
            Ok(answer) if answer.status == "approved" && answer.token.is_some() => {
                let record = ServerRecord {
                    address: link.server.clone(),
                    signs_in: true,
                    username: answer.username.clone(),
                    saves_documents: answer.saves_documents.unwrap_or(false),
                    expires_at: answer.expires_at,
                };
                self.servers.link = Some(LinkConnect {
                    record,
                    token: answer.token.unwrap_or_default(),
                    target: link,
                });
                self.servers.open = true;
            }
            Ok(_) => self
                .say("The link's ticket has expired. Click Open in PSF-Studio in PSF Guard again."),
            Err(failure) => self.say(&format!("The link did not connect: {failure}")),
        }
    }

    /// The person accepted, or not, the account a link's ticket connected
    /// Studio as.
    fn accept_link(&mut self, accept: bool) {
        let Some(link) = self.servers.link.take() else {
            return;
        };
        if !accept {
            let client = Client::new(&link.record.address, Some(link.token));
            std::thread::spawn(move || {
                let _ = client.disconnect();
            });
            self.servers.open = false;
            return;
        }
        if let Err(reason) = keep_token(&link.record.address, &link.token) {
            self.servers.message = Some(reason);
            return;
        }
        self.servers.keep_server(link.record);
        self.servers.open = false;
        let which = link
            .target
            .document
            .clone()
            .map_or(Which::Latest, Which::Document);
        self.open_server_document(link.target, which, false);
    }

    fn panel_button(
        label: impl Into<SharedString>,
        id: impl Into<gpui::ElementId>,
        enabled: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let button = div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(CONTROL))
            .child(label.into());
        if enabled {
            button
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    action(this, window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
        } else {
            button.text_color(rgb(QUIET))
        }
    }

    fn panel_row(
        id: impl Into<gpui::ElementId>,
        chosen: bool,
        title: impl Into<SharedString>,
        detail: Option<String>,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .when(chosen, |row| row.bg(rgb(SELECTED)))
            .child(div().child(title.into()))
            .when_some(detail, |row, detail| row.child(Self::note(detail)))
            .on_click(cx.listener(move |this, _, _, cx| {
                action(this);
                cx.stop_propagation();
                cx.notify();
            }))
    }

    fn panel_column(
        id: &'static str,
        heading: &'static str,
        rows: Vec<gpui::AnyElement>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex_1()
            .min_w(px(160.0))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_l_1()
            .border_color(rgb(CONTROL))
            .child(Self::heading(heading))
            .children(rows)
    }

    /// Rows for a list being fetched: its items, or why there are none.
    fn load_rows<T>(
        load: &Load<Vec<T>>,
        empty: &str,
        mut row: impl FnMut(&T) -> gpui::AnyElement,
    ) -> Vec<gpui::AnyElement> {
        match load {
            Load::Idle => Vec::new(),
            Load::Loading(_) => vec![Self::note("Loading…").into_any_element()],
            Load::Done(Err(_)) => vec![Self::note("Not loaded").into_any_element()],
            Load::Done(Ok(items)) if items.is_empty() => {
                vec![Self::note(empty.to_string()).into_any_element()]
            }
            Load::Done(Ok(items)) => items.iter().map(&mut row).collect(),
        }
    }

    fn address_field(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let focus = self
            .servers
            .address_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let text = self.servers.address.clone();
        let focused = focus.is_focused(window);
        div()
            .id("server-address")
            .track_focus(&focus)
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(CONTROL))
            .border_1()
            .border_color(rgb(if focused { SELECTED } else { CONTROL }))
            .cursor_text()
            .child(if text.is_empty() {
                div()
                    .text_color(rgb(QUIET))
                    .child("Server address, such as localhost:3000")
            } else {
                div().child(format!("{text}{}", if focused { "▏" } else { "" }))
            })
            .on_click(cx.listener(move |_, _, window, cx| {
                focus.focus(window, cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let keystroke = &event.keystroke;
                let field = &mut this.servers.address;
                if keystroke.modifiers.platform && keystroke.key == "v" {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        field.push_str(text.trim());
                    }
                } else if keystroke.modifiers.platform && keystroke.key == "c" {
                    cx.write_to_clipboard(ClipboardItem::new_string(field.clone()));
                } else if keystroke.key == "backspace" {
                    if keystroke.modifiers.alt || keystroke.modifiers.platform {
                        field.clear();
                    } else {
                        field.pop();
                    }
                } else if keystroke.key == "enter" {
                    if this.servers.connecting.is_none() {
                        this.start_connect();
                    }
                } else if keystroke.key == "escape" {
                    this.servers.open = false;
                } else if !keystroke.modifiers.platform
                    && !keystroke.modifiers.control
                    && let Some(typed) = &keystroke.key_char
                {
                    field.push_str(typed);
                } else {
                    return;
                }
                cx.stop_propagation();
                cx.notify();
            }))
    }

    /// The panel, over the canvas, when it is open.
    pub(super) fn server_panel(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.servers.open {
            return None;
        }
        let card = if self.servers.link.is_some() {
            self.link_card(cx)
        } else if self.servers.reframed.is_some() {
            self.reframed_card(cx)
        } else {
            self.browse_card(window, cx)
        };
        Some(
            div()
                .id("server-panel-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x0000_0099))
                .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                .child(card)
                .into_any_element(),
        )
    }

    fn reframed_card(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .w(px(520.0))
            .p_4()
            .rounded_md()
            .bg(rgb(PANE))
            .text_color(rgb(TEXT))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Open this document with the new stacks?"),
            )
            .child(Self::note(super::server_documents::REFRAMED))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(Self::panel_button(
                        "Open with the new stacks",
                        "reframed-open",
                        true,
                        cx,
                        |this, _, _| {
                            if let Some((target, which, discard)) = this.servers.reframed.take() {
                                this.servers.open = false;
                                this.open_server_document_with(target, which, discard, true);
                            }
                        },
                    ))
                    .child(Self::panel_button(
                        "Cancel",
                        "reframed-cancel",
                        true,
                        cx,
                        |this, _, _| {
                            this.servers.reframed = None;
                        },
                    )),
            )
            .into_any_element()
    }

    fn link_card(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(link) = &self.servers.link else {
            return div().into_any_element();
        };
        let account = link.record.username.clone().unwrap_or_default();
        div()
            .w(px(460.0))
            .p_4()
            .rounded_md()
            .bg(rgb(PANE))
            .text_color(rgb(TEXT))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("Connect to {} as {account}?", link.record.address)),
            )
            .child(Self::note(
                "A PSF Guard link asks Studio to connect to this server. Connect only if you \
                 opened the link yourself.",
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(Self::panel_button(
                        "Connect",
                        "link-accept",
                        true,
                        cx,
                        |this, _, _| this.accept_link(true),
                    ))
                    .child(Self::panel_button(
                        "Cancel",
                        "link-cancel",
                        true,
                        cx,
                        |this, _, _| this.accept_link(false),
                    )),
            )
            .into_any_element()
    }

    fn browse_card(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let panel = &self.servers;
        let chosen_server = panel.server.clone();
        let server_rows: Vec<gpui::AnyElement> = panel
            .list
            .servers
            .iter()
            .enumerate()
            .map(|(index, record)| {
                let address = record.address.clone();
                let detail = match (&record.username, record.signs_in) {
                    (Some(username), _) if record.saves_documents => Some(username.clone()),
                    (Some(username), _) => Some(format!("{username}, read-only")),
                    (None, false) => Some("No sign-in".to_string()),
                    (None, true) => None,
                };
                Self::panel_row(
                    ("server", index),
                    chosen_server.as_deref() == Some(address.as_str()),
                    record.address.clone(),
                    detail,
                    cx,
                    move |this| this.choose_server(address.clone()),
                )
                .into_any_element()
            })
            .collect();
        let connecting = panel.connecting.as_ref().map(ConnectRun::state);
        let connect_block = match connecting {
            Some(Connecting::Waiting { code, approve_url }) => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(Self::note(
                    "Allow Studio on the page that opened in your browser. Check that it shows \
                     this code:",
                ))
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(code),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(Self::panel_button(
                            "Open the page again",
                            "connect-reopen",
                            true,
                            cx,
                            move |_, _, _| open_in_browser(&approve_url),
                        ))
                        .child(Self::panel_button(
                            "Cancel",
                            "connect-cancel",
                            true,
                            cx,
                            |this, _, _| this.cancel_connect(),
                        )),
                ),
            Some(_) => div()
                .flex()
                .gap_2()
                .items_center()
                .child(Self::note("Connecting…"))
                .child(Self::panel_button(
                    "Cancel",
                    "connect-cancel",
                    true,
                    cx,
                    |this, _, _| this.cancel_connect(),
                )),
            None => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(self.address_field(window, cx))
                .child(Self::panel_button(
                    "Connect",
                    "connect",
                    true,
                    cx,
                    |this, _, _| this.start_connect(),
                )),
        };
        let panel = &self.servers;
        let chosen_database = panel.database.clone();
        let database_rows = Self::load_rows(&panel.databases, "No databases", |database| {
            let id = database.id.clone();
            Self::panel_row(
                SharedString::from(format!("database-{id}")),
                chosen_database.as_deref() == Some(id.as_str()),
                database.name.clone(),
                None,
                cx,
                move |this| this.choose_database(id.clone()),
            )
            .into_any_element()
        });
        let chosen_project = panel.project;
        let project_rows = Self::load_rows(&panel.projects, "No projects", |project| {
            let id = project.id;
            Self::panel_row(
                ("project", id as usize),
                chosen_project == Some(id),
                project.display_name.clone(),
                None,
                cx,
                move |this| this.choose_project(id),
            )
            .into_any_element()
        });
        let chosen_target = panel.target;
        let (targets, stacked) = panel.targets.done().cloned().unwrap_or_default();
        let target_rows: Vec<gpui::AnyElement> = match &panel.targets {
            Load::Done(Ok(_)) if targets.is_empty() => {
                vec![Self::note("No targets").into_any_element()]
            }
            Load::Done(Ok(_)) => targets
                .iter()
                .map(|target| {
                    let id = target.id;
                    let stacks = stacked.iter().find(|stacked| stacked.target_id == id);
                    let detail = stacks
                        .filter(|stacks| !stacks.source_candidates.is_empty())
                        .map_or_else(
                            || "No channel stacks yet".to_string(),
                            |stacks| {
                                stacks
                                    .source_candidates
                                    .iter()
                                    .map(|stack| {
                                        format!("{} {}", stack.filter_name, stack.accepted_frames)
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" · ")
                            },
                        );
                    Self::panel_row(
                        ("target", id as usize),
                        chosen_target == Some(id),
                        target.name.clone(),
                        Some(detail),
                        cx,
                        move |this| this.choose_target(id),
                    )
                    .into_any_element()
                })
                .collect(),
            Load::Idle => Vec::new(),
            Load::Loading(_) => vec![Self::note("Loading…").into_any_element()],
            Load::Done(Err(_)) => vec![Self::note("Not loaded").into_any_element()],
        };
        let chosen_document = panel.document.clone();
        let mut document_rows = vec![Self::panel_row(
            "document-new",
            chosen_document == Which::New,
            "New document",
            Some("From PSF Guard's flow".to_string()),
            cx,
            |this| this.servers.document = Which::New,
        )
        .into_any_element()];
        let latest = panel
            .documents
            .done()
            .and_then(|documents| documents.first().map(|d| d.id.clone()));
        document_rows.extend(Self::load_rows(&panel.documents, "", |document| {
            let id = document.id.clone();
            let chosen = match &chosen_document {
                Which::Document(chosen) => *chosen == id,
                Which::Latest => latest.as_deref() == Some(id.as_str()),
                Which::New => false,
            };
            Self::panel_row(
                SharedString::from(format!("document-{id}")),
                chosen,
                document.name.clone(),
                Some(format!(
                    "Revision {}, saved by {}",
                    document.revision, document.saved_by
                )),
                cx,
                move |this| this.servers.document = Which::Document(id.clone()),
            )
            .into_any_element()
        }));
        let can_open = chosen_target.is_some_and(|id| {
            stacked
                .iter()
                .any(|stacks| stacks.target_id == id && !stacks.source_candidates.is_empty())
        }) && !self.is_running();
        // A picture opened from files can go to the chosen target.
        let can_save_picture = chosen_target.is_some()
            && self.document_path().is_some()
            && !self.server_save.linked()
            && chosen_server
                .as_deref()
                .and_then(|server| self.servers.record(server))
                .is_some_and(|record| record.saves_documents);
        let signed_out = chosen_server.as_ref().is_some_and(|server| {
            self.servers.databases.failure() == Some(&Failure::SignedOut)
                && self.servers.list.find(server).is_some()
        });
        let message = self.servers.message.clone();
        div()
            .w(px(1100.0))
            .h(px(620.0))
            .rounded_md()
            .bg(rgb(PANE))
            .text_color(rgb(TEXT))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(CONTROL))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("PSF Guard"),
                    )
                    .child(Self::panel_button(
                        "Close",
                        "servers-close",
                        true,
                        cx,
                        |this, _, _| this.servers.open = false,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("servers")
                            .w(px(260.0))
                            .flex_none()
                            .h_full()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_2()
                            .child(Self::heading("Servers"))
                            .children(server_rows)
                            .when(self.servers.list.servers.is_empty(), |column| {
                                column.child(Self::note("No servers yet."))
                            })
                            .when_some(chosen_server.clone(), |column, server| {
                                column.child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .pt_1()
                                        .when(signed_out, |row| {
                                            let again = server.clone();
                                            row.child(Self::panel_button(
                                                "Connect again",
                                                "server-again",
                                                true,
                                                cx,
                                                move |this, _, _| {
                                                    this.servers.address = again.clone();
                                                    this.start_connect();
                                                },
                                            ))
                                        })
                                        .child(Self::panel_button(
                                            "Sign out",
                                            "server-sign-out",
                                            true,
                                            cx,
                                            move |this, _, _| this.sign_out(server.clone()),
                                        )),
                                )
                            })
                            .child(Self::heading("Connect to a server"))
                            .child(connect_block),
                    )
                    .child(Self::panel_column("databases", "Databases", database_rows))
                    .child(Self::panel_column("projects", "Projects", project_rows))
                    .child(Self::panel_column("targets", "Targets", target_rows))
                    .when(chosen_target.is_some(), |columns| {
                        columns.child(Self::panel_column("documents", "Documents", document_rows))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(CONTROL))
                    .child(
                        div()
                            .flex_1()
                            .child(Self::note(message.unwrap_or_default())),
                    )
                    .child(Self::panel_button(
                        "Save this picture to the target",
                        "servers-save-picture",
                        can_save_picture,
                        cx,
                        |this, _, _| this.save_picture_to_chosen(),
                    ))
                    .child(Self::panel_button(
                        "Open",
                        "servers-open",
                        can_open,
                        cx,
                        |this, _, _| this.open_chosen_target(),
                    )),
            )
            .into_any_element()
    }
}

/// The control socket's view of the panel, for `psf-studio-mcp`.
impl Studio {
    /// The servers, the connect under way, and what is chosen and listed.
    pub(super) fn control_servers(&self) -> serde_json::Value {
        use serde_json::json;
        let panel = &self.servers;
        let connecting = panel.connecting.as_ref().map(|run| match run.state() {
            Connecting::Checking => json!({ "state": "checking" }),
            Connecting::Waiting { code, approve_url } => {
                json!({ "state": "waiting", "code": code, "approve_url": approve_url })
            }
            Connecting::Connected(record) => json!({ "state": "connected", "server": record }),
            Connecting::Failed(reason) => json!({ "state": "failed", "reason": reason }),
        });
        let listed = |failure: Option<&Failure>, value: serde_json::Value| match failure {
            Some(failure) => json!({ "error": failure.to_string() }),
            None => value,
        };
        json!({
            "open": panel.open,
            "servers": panel.list.servers,
            "connecting": connecting,
            "message": panel.message,
            "server": panel.server,
            "databases": listed(
                panel.databases.failure(),
                json!(panel.databases.done().map(|list| list
                    .iter()
                    .map(|database| json!({ "id": database.id, "name": database.name }))
                    .collect::<Vec<_>>()))
            ),
            "database": panel.database,
            "projects": listed(
                panel.projects.failure(),
                json!(panel.projects.done().map(|list| list
                    .iter()
                    .map(|project| json!({ "id": project.id, "name": project.display_name }))
                    .collect::<Vec<_>>()))
            ),
            "project": panel.project,
            "targets": listed(
                panel.targets.failure(),
                json!(panel.targets.done().map(|(targets, stacked)| targets
                    .iter()
                    .map(|target| {
                        let channels: Vec<_> = stacked
                            .iter()
                            .find(|stacks| stacks.target_id == target.id)
                            .map(|stacks| {
                                stacks
                                    .source_candidates
                                    .iter()
                                    .map(|stack| stack.filter_name.clone())
                                    .collect()
                            })
                            .unwrap_or_default();
                        json!({ "id": target.id, "name": target.name, "channels": channels })
                    })
                    .collect::<Vec<_>>()))
            ),
            "target": panel.target,
            "documents": listed(
                panel.documents.failure(),
                json!(panel.documents.done().map(|list| list
                    .iter()
                    .map(|document| json!({
                        "id": document.id,
                        "name": document.name,
                        "revision": document.revision,
                        "saved_by": document.saved_by,
                    }))
                    .collect::<Vec<_>>()))
            ),
            "document": match &panel.document {
                Which::Latest => json!("latest"),
                Which::New => json!("new"),
                Which::Document(id) => json!(id),
            },
            "picture_on_server": self.server_save.linked(),
            "save_state": self.server_save_line(),
        })
    }

    /// Puts the open picture on the chosen target, as **Save this picture
    /// to the target** does.
    pub(super) fn control_save_picture(&mut self) -> Result<serde_json::Value, String> {
        let target = self
            .chosen_target()
            .ok_or_else(|| "No target is chosen".to_string())?;
        self.save_picture_to(&target)?;
        Ok(self.control_servers())
    }

    /// Opens the panel and connects to `address`, as typing it and choosing
    /// Connect does.
    pub(super) fn control_connect(&mut self, address: &str) -> serde_json::Value {
        self.servers.open = true;
        self.servers.address = address.to_string();
        self.start_connect();
        self.control_servers()
    }

    /// Chooses a server, a database, a project and a target, as clicking
    /// them does. Each is chosen only when it is given; a database, project
    /// or target is chosen once its list has loaded.
    pub(super) fn control_browse(
        &mut self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        if let Some(server) = params["server"].as_str() {
            let address = normal_address(server)?;
            if self.servers.list.find(&address).is_none() {
                return Err(format!("Studio is not connected to {address}"));
            }
            self.servers.open = true;
            self.choose_server(address);
        }
        if let Some(database) = params["database"].as_str() {
            self.choose_database(database.to_string());
        }
        if let Some(project) = params["project"].as_i64() {
            self.choose_project(project as i32);
        }
        if let Some(target) = params["target"].as_i64() {
            self.choose_target(target as i32);
        }
        match params["document"].as_str() {
            Some("latest") => self.servers.document = Which::Latest,
            Some("new") => self.servers.document = Which::New,
            Some(id) => self.servers.document = Which::Document(id.to_string()),
            None => {}
        }
        Ok(self.control_servers())
    }

    /// Signs out of a server, as **Sign out** does.
    pub(super) fn control_sign_out(&mut self, address: &str) -> Result<serde_json::Value, String> {
        let address = normal_address(address)?;
        if self.servers.list.find(&address).is_none() {
            return Err(format!("Studio is not connected to {address}"));
        }
        self.sign_out(address);
        Ok(self.control_servers())
    }

    /// Opens the chosen target, as **Open** does.
    pub(super) fn control_open_server_target(&mut self) -> Result<serde_json::Value, String> {
        if self.servers.target.is_none() {
            return Err("No target is chosen".to_string());
        }
        if self.is_running() {
            return Err("A picture is being prepared or tools are running".to_string());
        }
        self.open_chosen_target();
        Ok(serde_json::json!({ "opening": true }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_load_takes_in_its_result_once() {
        let mut load = Load::start(|| Ok::<_, Failure>(vec![1, 2]));
        let mut finished = false;
        for _ in 0..200 {
            if load.follow() {
                finished = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(finished);
        assert_eq!(load.done(), Some(&vec![1, 2]));
        assert!(!load.follow());
    }
}
