//! `psf-studio-mcp`: a Model Context Protocol server, over standard input
//! and output, that drives PSF Studio through each window's control socket
//! (`$TMPDIR/psf-studio/<pid>.sock`, see `studio/app/src/control.rs`).
//!
//! An assistant can open a Studio window on a copy of a document, read what
//! it shows, select rows, change settings, wait until the canvas has caught
//! up, and see the canvas or the final picture's values as images. It never
//! closes a window it did not open.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The MCP protocol version this server speaks.
const PROTOCOL: &str = "2025-06-18";

fn main() {
    let mut server = Server::default();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // A notification has no id and gets no answer.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message["method"].as_str().unwrap_or_default();
        let answer = match method {
            "initialize" => Ok(json!({
                "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "psf-studio", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Drives PSF Studio windows. Open a window on a copy of a document with studio_open, read it with studio_state, change it with studio_select, studio_set_step and studio_set_layer, call studio_wait_idle, then look with studio_capture or measure with studio_sample. Never edit a window the person is working in unless they ask.",
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(server.call(&message["params"])),
            other => Err(json!({ "code": -32601, "message": format!("unknown method {other}") })),
        };
        let reply = match answer {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        };
        if writeln!(stdout, "{reply}")
            .and_then(|()| stdout.flush())
            .is_err()
        {
            break;
        }
    }
}

fn tools() -> Value {
    let instance = json!({
        "type": "integer",
        "description": "The Studio window's process id, from studio_list. Leave it out for the window this server opened last, or the only one running."
    });
    let object = |properties: Value, required: &[&str]| json!({ "type": "object", "properties": properties, "required": required });
    json!([
        {
            "name": "studio_list",
            "description": "Lists the running PSF Studio windows: process id, document, store and title.",
            "inputSchema": object(json!({}), &[]),
        },
        {
            "name": "studio_open",
            "description": "Opens a new PSF Studio window on a store and a document, in the background, and waits until it answers. By default it opens a copy of the document beside it, so the person's file is not changed.",
            "inputSchema": object(json!({
                "store": { "type": "string", "description": "The .pgtiles store." },
                "document": { "type": "string", "description": "The document JSON. Leave out for the document beside the store." },
                "copy": { "type": "boolean", "description": "Open a copy of the document (default true)." },
                "output": { "type": "string", "description": "srgb8, p3-8, p3-float or p3-extended (default p3-extended)." }
            }), &["store"]),
        },
        {
            "name": "studio_close",
            "description": "Closes a Studio window this server opened. Refuses any other window.",
            "inputSchema": object(json!({ "instance": instance }), &[]),
        },
        {
            "name": "studio_state",
            "description": "What a Studio window shows: the rows to select (inputs with their steps, layers top first, Finish steps) with their ids and settings, the selection, each pane's label (the place wanted and the place drawn), the status line, the inputs whose tools changed since the store was prepared, the histogram's clipping, and what it is still busy with.",
            "inputSchema": object(json!({
                "instance": instance,
                "settings": { "type": "boolean", "description": "Include every step's settings (default true)." }
            }), &[]),
        },
        {
            "name": "studio_select",
            "description": "Selects a row, as a click in the tree does. `row` is a row value from studio_state, such as {\"input\": \"oiii\"}, {\"input_step\": {\"channel\": \"oiii\", \"step\": \"<id>\"}}, {\"layer\": \"<id>\"}, {\"finish\": \"<id>\"}, \"combined\" or \"final\".",
            "inputSchema": object(json!({ "instance": instance, "row": {} }), &["row"]),
        },
        {
            "name": "studio_set_split",
            "description": "Divides the canvas: one, with_picture, before_and_after or wipe.",
            "inputSchema": object(json!({ "instance": instance, "split": { "type": "string" } }), &["split"]),
        },
        {
            "name": "studio_set_step",
            "description": "Changes a step: `settings` is merged into its settings, key by key (for a stretch, {\"model\": {\"target_background\": 0.12}}). A null removes its key, and a stretch whose `type` changes starts from that kind's settings, as the inspector's choice does ({\"model\": {\"type\": \"auto-mtf\"}} switches a stretch to automatic midtones). `enabled` turns it on or off. The document is saved as Studio saves every edit.",
            "inputSchema": object(json!({
                "instance": instance,
                "step": { "type": "string" },
                "settings": { "type": "object" },
                "enabled": { "type": "boolean" }
            }), &["step"]),
        },
        {
            "name": "studio_set_layer",
            "description": "Changes a layer: `patch` is merged into the layer as the document stores it, such as {\"opacity\": 0.5}, {\"visible\": false}, {\"blend\": \"screen\"} or {\"star_reduction\": 0.5}. A null removes its key.",
            "inputSchema": object(json!({
                "instance": instance,
                "layer": { "type": "string" },
                "patch": { "type": "object" }
            }), &["layer", "patch"]),
        },
        {
            "name": "studio_undo",
            "description": "Undoes the last edit of the document, as Undo (Cmd+Z) does. Edits in quick succession, such as one slider drag, are one edit. Returns whether more can be undone and redone.",
            "inputSchema": object(json!({ "instance": instance }), &[]),
        },
        {
            "name": "studio_redo",
            "description": "Makes the last edit undone again, as Redo (Shift+Cmd+Z) does.",
            "inputSchema": object(json!({ "instance": instance }), &[]),
        },
        {
            "name": "studio_add_tool",
            "description": "Adds a processing tool as a step of its own: `band` is an input, such as \"luminance\", or a color group, \"rgb\" or \"lines\"; `provider` is a tool key from studio-tools, such as \"bxt\", \"nxt\", \"noise-model\" or \"sxt\"; `parameters` its settings. It goes after the band's tools of an earlier kind, else before Background extraction. Returns the step's id. It takes effect when the tools run again (studio_run_tools).",
            "inputSchema": object(json!({
                "instance": instance,
                "band": { "type": "string" },
                "provider": { "type": "string" },
                "parameters": { "type": "object" }
            }), &["band", "provider"]),
        },
        {
            "name": "studio_move_step",
            "description": "Moves a Linear step that changes an input, such as a tool or Background extraction, past the step before it (`later`: false) or after it (`later`: true) in `channel`'s list, as Move earlier and Move later do. Preparing the store runs the steps in that order, so a tool moved after Background extraction runs on the corrected stack. It takes effect when the tools run again.",
            "inputSchema": object(json!({
                "instance": instance,
                "step": { "type": "string" },
                "channel": { "type": "string" },
                "later": { "type": "boolean" }
            }), &["step", "channel"]),
        },
        {
            "name": "studio_add_step",
            "description": "Adds a step at the end of a stage, as the inspector's Add step does. `stage` is sources, linear, stretch, combine, finish or export; `operation` the step's name, such as \"Tone and color\", \"Curves\" or \"Stretch\"; `band` the input it is for, when it is one input's; `settings` its settings; `mask` a mask as the document stores it, such as {\"kind\": \"range\", \"low\": 0.5, \"high\": 1.0, \"feather\": 0.1}. Returns the step's id.",
            "inputSchema": object(json!({
                "instance": instance,
                "stage": { "type": "string" },
                "operation": { "type": "string" },
                "band": { "type": "string" },
                "settings": {},
                "mask": {}
            }), &["stage", "operation"]),
        },
        {
            "name": "studio_remove_step",
            "description": "Removes a step from the document by its id.",
            "inputSchema": object(json!({ "instance": instance, "step": { "type": "string" } }), &["step"]),
        },
        {
            "name": "studio_set_headroom",
            "description": "Makes the picture HDR with this many stops of headroom above SDR white (0 to 4), or SDR with 0, as the Final picture's HDR setting does. `knee`, from 0.3 to 0.95, sets where every stretch but the HDR layered one continues into the headroom, as the stretch's output (0.75 when never set).",
            "inputSchema": object(json!({ "instance": instance, "stops": { "type": "number" }, "knee": { "type": "number" } }), &["stops"]),
        },
        {
            "name": "studio_export",
            "description": "Exports the final picture to `path`, as Export… does, in `format` (tif, png, avif, fits or xisf; leave out for the picture's own: AVIF when it is HDR). A path with no extension takes the format's. Follow it with studio_wait_idle, then studio_state's export status.",
            "inputSchema": object(json!({
                "instance": instance,
                "path": { "type": "string" },
                "format": { "type": "string" }
            }), &["path"]),
        },
        {
            "name": "studio_show_clipping",
            "description": "Marks pixels at their pane's peak with magenta and black stripes, as the Clipping toggle does, so clipped areas such as an HDR core show in a capture; `on` false turns it off.",
            "inputSchema": object(json!({ "instance": instance, "on": { "type": "boolean" } }), &[]),
        },
        {
            "name": "studio_move_layer",
            "description": "Moves a layer one place up (`up` true, the default) or down the stack, as Move up and Move down do: a group's layer within its group, a top-level layer or group past the next one, a group with its layers.",
            "inputSchema": object(json!({ "instance": instance, "layer": { "type": "string" }, "up": { "type": "boolean" } }), &["layer"]),
        },
        {
            "name": "studio_use_recipe",
            "description": "Makes the picture from a recipe, as the Recipe menu does, such as \"LRGB\" or \"Narrowband with RGB stars\". A recipe that shows stars apart has them separated, which runs the tools.",
            "inputSchema": object(json!({ "instance": instance, "name": { "type": "string" } }), &["name"]),
        },
        {
            "name": "studio_add_part",
            "description": "Adds a part, as the Add part menu does: \"RGB stars\", \"Stars\", \"Nebula\", \"Color\", \"Luminance\" or \"Lines\". A part that shows stars apart has them separated, which runs the tools.",
            "inputSchema": object(json!({ "instance": instance, "kind": { "type": "string" } }), &["kind"]),
        },
        {
            "name": "studio_set_tool",
            "description": "Chooses an input's processing tool of one kind: `category` sharpen, denoise or star_removal, `provider` such as bxt, nxt, noise-model or sxt (leave out for none). Run the tools to apply it.",
            "inputSchema": object(json!({
                "instance": instance,
                "input": { "type": "string" },
                "category": { "type": "string" },
                "provider": { "type": "string" }
            }), &["input", "category"]),
        },
        {
            "name": "studio_run_tools",
            "description": "Runs every input's processing tools and prepares the store again, as Run tools does.",
            "inputSchema": object(json!({ "instance": instance }), &[]),
        },
        {
            "name": "studio_wait_idle",
            "description": "Waits until the canvas shows the document as it is: scenes built, panes showing their places, tiles loaded, samplings read and no task running. Returns what it still waits for if the time runs out.",
            "inputSchema": object(json!({
                "instance": instance,
                "timeout_seconds": { "type": "number", "description": "Default 60." }
            }), &[]),
        },
        {
            "name": "studio_capture",
            "description": "Shows the canvas as drawn, one pane or all of them, as an image. An HDR canvas's values above SDR white clip unless `divide` (such as the picture's peak) scales them down first. Waits until idle first unless `wait` is false.",
            "inputSchema": object(json!({
                "instance": instance,
                "pane": { "type": "integer", "description": "The pane, from 0, as studio_state lists them. Leave out for the whole canvas." },
                "divide": { "type": "number", "description": "Divide the displayed values by this before showing them (default 1)." },
                "max_width": { "type": "integer", "description": "Widest image to return, in pixels (default 1200)." },
                "wait": { "type": "boolean" }
            }), &[]),
        },
        {
            "name": "studio_sample",
            "description": "Renders a region of the final picture on the CPU at full resolution, as an export does, and returns each channel's values (min, median, 99th percentile, max, the share above SDR white and at the picture's peak) and an image of it. Use it to check detail above SDR white, such as an HDR core.",
            "inputSchema": object(json!({
                "instance": instance,
                "x": { "type": "integer" }, "y": { "type": "integer" },
                "width": { "type": "integer", "description": "Default 256, at most 2048." },
                "height": { "type": "integer", "description": "Default 256, at most 2048." },
                "divide": { "type": "number", "description": "Divide the values by this for the image (default 1; the picture's peak shows everything)." }
            }), &["x", "y"]),
        },
    ])
}

#[derive(Default)]
struct Server {
    /// The windows this server opened, last opened last.
    opened: Vec<u32>,
}

impl Server {
    fn call(&mut self, params: &Value) -> Value {
        let name = params["name"].as_str().unwrap_or_default();
        let arguments = &params["arguments"];
        match self.tool(name, arguments) {
            Ok(content) => json!({ "content": content }),
            Err(error) => json!({ "content": [text(&error)], "isError": true }),
        }
    }

    fn tool(&mut self, name: &str, arguments: &Value) -> Result<Vec<Value>, String> {
        match name {
            "studio_list" => Ok(vec![text(
                &serde_json::to_string_pretty(&list()).unwrap_or_default(),
            )]),
            "studio_open" => self.open(arguments).map(|info| vec![pretty(&info)]),
            "studio_close" => self.close(arguments),
            "studio_state" => {
                let mut state = request(self.instance(arguments)?, "state", json!({}))?;
                if arguments["settings"].as_bool() == Some(false) {
                    strip_settings(&mut state);
                }
                Ok(vec![pretty(&state)])
            }
            "studio_select" => forward(
                self.instance(arguments)?,
                "select",
                json!({ "row": arguments["row"] }),
            ),
            "studio_set_split" => forward(
                self.instance(arguments)?,
                "set_split",
                json!({ "split": arguments["split"] }),
            ),
            "studio_set_step" => forward(self.instance(arguments)?, "set_step", arguments.clone()),
            "studio_set_layer" => {
                forward(self.instance(arguments)?, "set_layer", arguments.clone())
            }
            "studio_undo" => forward(self.instance(arguments)?, "undo", json!({})),
            "studio_redo" => forward(self.instance(arguments)?, "redo", json!({})),
            "studio_add_tool" => forward(
                self.instance(arguments)?,
                "add_tool",
                json!({
                    "band": arguments["band"],
                    "provider": arguments["provider"],
                    "parameters": arguments.get("parameters").cloned().unwrap_or(Value::Null),
                }),
            ),
            "studio_move_step" => forward(
                self.instance(arguments)?,
                "move_step",
                json!({
                    "step": arguments["step"],
                    "channel": arguments["channel"],
                    "later": arguments["later"],
                }),
            ),
            "studio_add_step" => forward(
                self.instance(arguments)?,
                "add_step",
                json!({
                    "stage": arguments["stage"],
                    "operation": arguments["operation"],
                    "band": arguments["band"],
                    "settings": arguments.get("settings").cloned().unwrap_or(Value::Null),
                    "mask": arguments.get("mask").cloned().unwrap_or(Value::Null),
                }),
            ),
            "studio_remove_step" => forward(
                self.instance(arguments)?,
                "remove_step",
                json!({ "step": arguments["step"] }),
            ),
            "studio_set_headroom" => forward(
                self.instance(arguments)?,
                "set_headroom",
                json!({ "stops": arguments["stops"], "knee": arguments["knee"] }),
            ),
            "studio_export" => forward(self.instance(arguments)?, "export", arguments.clone()),
            "studio_show_clipping" => forward(
                self.instance(arguments)?,
                "show_clipping",
                json!({ "on": arguments["on"].as_bool().unwrap_or(true) }),
            ),
            "studio_move_layer" => {
                forward(self.instance(arguments)?, "move_layer", arguments.clone())
            }
            "studio_use_recipe" => forward(
                self.instance(arguments)?,
                "use_recipe",
                json!({ "name": arguments["name"] }),
            ),
            "studio_add_part" => forward(
                self.instance(arguments)?,
                "add_part",
                json!({ "kind": arguments["kind"] }),
            ),
            "studio_set_tool" => forward(self.instance(arguments)?, "set_tool", arguments.clone()),
            "studio_run_tools" => forward(self.instance(arguments)?, "run_tools", json!({})),
            "studio_wait_idle" => {
                let pid = self.instance(arguments)?;
                let timeout = arguments["timeout_seconds"].as_f64().unwrap_or(60.0);
                let waited = wait_idle(pid, Duration::from_secs_f64(timeout))?;
                Ok(vec![pretty(&waited)])
            }
            "studio_capture" => {
                let pid = self.instance(arguments)?;
                if arguments["wait"].as_bool() != Some(false) {
                    wait_idle(pid, Duration::from_secs(60))?;
                }
                let mut params = json!({
                    "divide": arguments["divide"].as_f64().unwrap_or(1.0),
                    "max_width": arguments["max_width"].as_u64().unwrap_or(1200),
                });
                if let Some(pane) = arguments["pane"].as_u64() {
                    params["pane"] = json!(pane);
                }
                let answer = request(pid, "capture", params)?;
                with_image(answer)
            }
            "studio_sample" => {
                let pid = self.instance(arguments)?;
                let answer = request(pid, "sample", arguments.clone())?;
                with_image(answer)
            }
            other => Err(format!("unknown tool {other}")),
        }
    }

    /// The window a call is for: the one it names, the one this server
    /// opened last, or the only one running.
    fn instance(&self, arguments: &Value) -> Result<u32, String> {
        if let Some(pid) = arguments["instance"].as_u64() {
            return Ok(pid as u32);
        }
        let running: Vec<u32> = list()
            .iter()
            .filter_map(|window| window["pid"].as_u64().map(|pid| pid as u32))
            .collect();
        if let Some(pid) = self.opened.iter().rev().find(|pid| running.contains(pid)) {
            return Ok(*pid);
        }
        match running.as_slice() {
            [only] => Ok(*only),
            [] => Err("no PSF Studio window is running; open one with studio_open".to_string()),
            more => Err(format!(
                "{} Studio windows are running ({more:?}); name one with `instance`",
                more.len()
            )),
        }
    }

    fn open(&mut self, arguments: &Value) -> Result<Value, String> {
        let store = PathBuf::from(arguments["store"].as_str().ok_or("`store` is needed")?);
        if !store.is_file() {
            return Err(format!("no store at {}", store.display()));
        }
        let document = arguments["document"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| store.with_extension("json"));
        let document = if arguments["copy"].as_bool() != Some(false) && document.is_file() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs());
            let copy = document.with_extension(format!("mcp-{stamp}.json"));
            std::fs::copy(&document, &copy)
                .map_err(|error| format!("could not copy {}: {error}", document.display()))?;
            copy
        } else {
            document
        };
        let program = studio_program()?;
        let log = control_dir().join(format!(
            "studio-{}.log",
            document
                .file_stem()
                .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
        ));
        let log = std::fs::File::create(&log).map_err(|error| error.to_string())?;
        let mut command = Command::new(&program);
        command
            .arg("--open")
            .arg(&store)
            .arg("--document")
            .arg(&document)
            .arg("--output")
            .arg(arguments["output"].as_str().unwrap_or("p3-extended"))
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|error| error.to_string())?)
            .stderr(log);
        if let Some(psf_guard) = psf_guard_program() {
            command.arg("--psf-guard").arg(psf_guard);
        }
        let child = command
            .spawn()
            .map_err(|error| format!("could not start {}: {error}", program.display()))?;
        let pid = child.id();
        self.opened.push(pid);
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(60) {
            if let Ok(info) = request(pid, "info", json!({})) {
                return Ok(info);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!("Studio (pid {pid}) did not answer within a minute"))
    }

    fn close(&mut self, arguments: &Value) -> Result<Vec<Value>, String> {
        let pid = self.instance(arguments)?;
        if !self.opened.contains(&pid) {
            return Err(format!(
                "Studio {pid} was not opened by this server; it may be a window a person is working in, so it is left open"
            ));
        }
        Command::new("kill")
            .arg(pid.to_string())
            .status()
            .map_err(|error| error.to_string())?;
        self.opened.retain(|opened| *opened != pid);
        let _ = std::fs::remove_file(socket_path(pid));
        Ok(vec![text(&format!("closed Studio {pid}"))])
    }
}

fn text(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn pretty(value: &Value) -> Value {
    text(&serde_json::to_string_pretty(value).unwrap_or_default())
}

/// A call's answer as text, and the PNG it names as an image.
fn with_image(answer: Value) -> Result<Vec<Value>, String> {
    let path = answer["png"].as_str().map(PathBuf::from);
    let mut content = vec![pretty(&answer)];
    if let Some(path) = path {
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        content.push(json!({ "type": "image", "data": base64(&bytes), "mimeType": "image/png" }));
    }
    Ok(content)
}

/// Sends a request and gives its answer as the call's text.
fn forward(pid: u32, method: &str, params: Value) -> Result<Vec<Value>, String> {
    request(pid, method, params).map(|answer| vec![pretty(&answer)])
}

/// Where the Studio windows' sockets are.
fn control_dir() -> PathBuf {
    std::env::var_os("PSF_STUDIO_CONTROL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("psf-studio"))
}

fn socket_path(pid: u32) -> PathBuf {
    control_dir().join(format!("{pid}.sock"))
}

/// One request to a Studio window, and its answer.
fn request(pid: u32, method: &str, params: Value) -> Result<Value, String> {
    let path = socket_path(pid);
    let mut stream = UnixStream::connect(&path)
        .map_err(|error| format!("could not reach Studio {pid}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(300)))
        .map_err(|error| error.to_string())?;
    writeln!(
        stream,
        "{}",
        json!({ "id": 1, "method": method, "params": params })
    )
    .map_err(|error| error.to_string())?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|error| format!("Studio {pid} did not answer: {error}"))?;
    let answer: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
    match answer.get("error") {
        Some(error) => Err(error
            .as_str()
            .unwrap_or("Studio refused the request")
            .to_string()),
        None => Ok(answer["result"].clone()),
    }
}

/// The windows that answer. A socket nothing listens on any more, from a
/// window that closed, is removed.
fn list() -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(control_dir()) else {
        return Vec::new();
    };
    let mut windows = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(pid) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".sock"))
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        match request(pid, "info", json!({})) {
            Ok(info) => windows.push(info),
            Err(_) if UnixStream::connect(&path).is_err() => {
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => {}
        }
    }
    windows
}

/// Polls a window until it reports nothing busy twice in a row.
fn wait_idle(pid: u32, timeout: Duration) -> Result<Value, String> {
    let started = Instant::now();
    let mut quiet = 0;
    loop {
        let busy = request(pid, "busy", json!({}))?;
        let idle = busy["busy"].as_array().is_some_and(Vec::is_empty);
        quiet = if idle { quiet + 1 } else { 0 };
        if quiet >= 2 {
            return Ok(json!({ "idle": true, "seconds": started.elapsed().as_secs_f32() }));
        }
        if started.elapsed() > timeout {
            return Ok(
                json!({ "idle": false, "still": busy["busy"], "seconds": started.elapsed().as_secs_f32() }),
            );
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Removes every step's settings from a state, for a shorter answer.
fn strip_settings(state: &mut Value) {
    if let Some(inputs) = state["inputs"].as_array_mut() {
        for input in inputs {
            if let Some(steps) = input["steps"].as_array_mut() {
                for step in steps {
                    step.as_object_mut().map(|step| step.remove("settings"));
                }
            }
        }
    }
    if let Some(finish) = state["finish"].as_array_mut() {
        for step in finish {
            step.as_object_mut().map(|step| step.remove("settings"));
        }
    }
}

/// The Studio program: `PSF_STUDIO_BIN`, or `psf-studio` beside this one.
fn studio_program() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("PSF_STUDIO_BIN") {
        return Ok(PathBuf::from(path));
    }
    let beside = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name("psf-studio");
    if beside.is_file() {
        Ok(beside)
    } else {
        Err("no psf-studio beside psf-studio-mcp; build it or set PSF_STUDIO_BIN".to_string())
    }
}

/// The `psf-guard` program Studio runs tools with: `PSF_GUARD`, or the
/// release build of the repository this program was built in.
fn psf_guard_program() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PSF_GUARD") {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    // studio/target/release/psf-studio-mcp → target/release/psf-guard
    let repository: &Path = exe.parent()?.parent()?.parent()?.parent()?;
    let guard = repository.join("target/release/psf-guard");
    guard.is_file().then_some(guard)
}

/// Standard base64, with padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[(value >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn every_tool_has_a_name_a_description_and_an_object_schema() {
        let tools = tools();
        for tool in tools.as_array().unwrap() {
            assert!(tool["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("studio_")));
            assert!(tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()));
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn settings_can_be_left_out_of_a_state() {
        let mut state = json!({
            "inputs": [{ "steps": [{ "name": "Stretch", "settings": { "model": {} } }] }],
            "finish": [{ "name": "Tone", "settings": {} }],
        });
        strip_settings(&mut state);
        assert!(state["inputs"][0]["steps"][0].get("settings").is_none());
        assert!(state["finish"][0].get("settings").is_none());
        assert_eq!(state["inputs"][0]["steps"][0]["name"], "Stretch");
    }
}
