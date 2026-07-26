//! End-to-end coverage of the `sync` commands, run as the built binary.
//!
//! Everything else tests the sync engine through library calls. These run
//! `psf-guard` itself, so argument parsing, registry and path resolution, the
//! read-only/read-write split, token precedence, and the exit codes are all
//! under test — the layer a user actually touches, and the one nothing
//! reached before.
//!
//! The remote test also starts a real `psf-guard server` and syncs against it
//! over HTTP, which is the whole point of the client: two machines, no shared
//! filesystem.

use std::{
    io::Write,
    net::TcpListener,
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

use rusqlite::Connection;
use tempfile::TempDir;

const TELESCOPE_TOKEN: &str = "cli-e2e-remote-sync-token-000001";

/// A night at the telescope: two frames, neither reviewed.
const TELESCOPE_ROWS: &str = "
    INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid)
        VALUES (1,'profile','Sync Nebula','from the mount',1,5,0,0,'project-guid');
    INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid)
        VALUES (1,'Sync Nebula Core',1,5.5,-5.4,0,1,'target-guid');
    INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid)
        VALUES (1,'profile','Ha 300','Ha',100,30,1,0,'template-guid');
    INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid)
        VALUES (1,'profile',300,40,2,2,1,1,1,'plan-guid');
    INSERT INTO ruleweight (Id,name,weight,projectid) VALUES (1,'Priority',2.5,1);
    INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid)
        VALUES (1,1,1,1750000000,'Ha',1,'{\"FileName\":\"one.fits\"}','profile',1,'image-one');
    INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid)
        VALUES (2,1,1,1750000600,'Ha',1,'{\"FileName\":\"two.fits\"}','profile',1,'image-two');";

/// The review copy: the same project, edited here, and one frame already
/// rejected by a reviewer.
const REVIEW_ROWS: &str = "
    INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid)
        VALUES (10,'profile','Sync Nebula','edited here',1,9,0,0,'project-guid');
    INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid)
        VALUES (10,'Sync Nebula Core',1,5.5,-5.4,0,10,'target-guid');
    INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid)
        VALUES (10,'profile','Ha 300','Ha',100,30,1,0,'template-guid');
    INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid)
        VALUES (10,'profile',300,60,0,0,10,10,1,'plan-guid');
    INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,exposureId,guid)
        VALUES (10,10,10,1750000000,'Ha',2,'{\"FileName\":\"one.fits\"}','reviewed here','profile',10,'image-one');";

fn scheduler_db(path: &Path, rows: &str) {
    let connection = Connection::open(path).unwrap();
    psf_guard::ts_schema::apply_schema(&connection).unwrap();
    connection.execute_batch(rows).unwrap();
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_psf-guard"))
        .args(args)
        .output()
        .expect("running psf-guard")
}

/// Run and insist it worked, showing both streams when it did not — a bare
/// exit code tells you nothing about which argument the CLI disliked.
fn run_ok(args: &[&str]) -> String {
    let output = run(args);
    assert!(
        output.status.success(),
        "psf-guard {args:?} failed ({})\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn query<T: rusqlite::types::FromSql>(path: &Path, sql: &str) -> T {
    Connection::open(path)
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

#[test]
fn the_local_sync_commands_move_captures_grades_and_planning() {
    let directory = TempDir::new().unwrap();
    let telescope = directory.path().join("telescope.sqlite");
    let review = directory.path().join("review.sqlite");
    scheduler_db(&telescope, TELESCOPE_ROWS);
    scheduler_db(&review, REVIEW_ROWS);
    let (telescope_arg, review_arg) = (path_arg(&telescope), path_arg(&review));

    // A dry run reports the plan and writes nothing.
    let planned = run_ok(&[
        "sync",
        "pull",
        "--from",
        &telescope_arg,
        "--to",
        &review_arg,
        "--dry-run",
    ]);
    assert!(planned.to_lowercase().contains("dry"), "{planned}");
    assert_eq!(count(&review, "acquiredimage"), 1, "a dry run wrote rows");

    run_ok(&[
        "sync",
        "pull",
        "--from",
        &telescope_arg,
        "--to",
        &review_arg,
    ]);
    assert_eq!(count(&review, "acquiredimage"), 2);
    assert_eq!(
        query::<i64>(
            &review,
            "SELECT gradingStatus FROM acquiredimage WHERE guid = 'image-one'"
        ),
        2,
        "a pull must keep the grade a reviewer set here"
    );
    assert_eq!(
        query::<String>(&review, "SELECT description FROM project"),
        "from the mount",
        "the telescope owns structure"
    );

    // Reject the second frame here, then send both decisions back.
    Connection::open(&review)
        .unwrap()
        .execute(
            "UPDATE acquiredimage SET gradingStatus = 2, rejectreason = 'cli reviewed' \
             WHERE guid = 'image-two'",
            [],
        )
        .unwrap();
    let pushed = run_ok(&[
        "sync",
        "grades",
        "--from",
        &review_arg,
        "--to",
        &telescope_arg,
    ]);
    assert!(pushed.to_lowercase().contains("changed"), "{pushed}");
    assert_eq!(
        query::<String>(
            &telescope,
            "SELECT rejectreason FROM acquiredimage WHERE guid = 'image-two'"
        ),
        "cli reviewed"
    );

    // Planning goes the same way, and must not disturb capture progress.
    Connection::open(&review)
        .unwrap()
        .execute("UPDATE exposureplan SET desired = 99", [])
        .unwrap();
    run_ok(&[
        "sync",
        "planning",
        "--from",
        &review_arg,
        "--to",
        &telescope_arg,
    ]);
    assert_eq!(
        query::<i64>(&telescope, "SELECT desired FROM exposureplan"),
        99
    );
    assert_eq!(
        query::<i64>(&telescope, "SELECT acquired FROM exposureplan"),
        2,
        "a planning push must not rewrite what the telescope has captured"
    );
}

#[test]
fn syncing_a_database_with_itself_is_refused() {
    let directory = TempDir::new().unwrap();
    let catalog = directory.path().join("catalog.sqlite");
    scheduler_db(&catalog, TELESCOPE_ROWS);
    let arg = path_arg(&catalog);

    let output = run(&["sync", "pull", "--from", &arg, "--to", &arg]);
    assert!(!output.status.success(), "syncing onto itself should fail");
    let message = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(message.contains("same database"), "{message}");
}

#[test]
fn the_remote_sync_command_talks_to_a_running_server() {
    let directory = TempDir::new().unwrap();
    let telescope = directory.path().join("telescope.sqlite");
    let review = directory.path().join("review.sqlite");
    scheduler_db(&telescope, TELESCOPE_ROWS);
    scheduler_db(&review, REVIEW_ROWS);
    let server = Server::start(directory.path(), &telescope);
    let review_arg = path_arg(&review);
    let token_file = directory.path().join("token");
    std::fs::write(&token_file, format!("{TELESCOPE_TOKEN}\n")).unwrap();
    let token_arg = path_arg(&token_file);

    // A dry run reaches the peer, reports, and writes nothing.
    let planned = run_ok(&[
        "sync",
        "remote",
        "--direction",
        "pull",
        "--local",
        &review_arg,
        "--peer",
        &server.url,
        "--token-file",
        &token_arg,
        "--dry-run",
    ]);
    assert!(planned.contains("PSF Guard"), "{planned}");
    assert_eq!(count(&review, "acquiredimage"), 1, "a dry run wrote rows");

    run_ok(&[
        "sync",
        "remote",
        "--direction",
        "pull",
        "--local",
        &review_arg,
        "--peer",
        &server.url,
        "--token-file",
        &token_arg,
    ]);
    assert_eq!(count(&review, "acquiredimage"), 2);
    assert_eq!(
        query::<i64>(
            &review,
            "SELECT gradingStatus FROM acquiredimage WHERE guid = 'image-one'"
        ),
        2,
        "a remote pull must keep the grade a reviewer set here"
    );

    // Send a decision back the other way, and read it off the server's database.
    Connection::open(&review)
        .unwrap()
        .execute(
            "UPDATE acquiredimage SET gradingStatus = 2, rejectreason = 'sent over http' \
             WHERE guid = 'image-two'",
            [],
        )
        .unwrap();
    run_ok(&[
        "sync",
        "remote",
        "--direction",
        "push-grades",
        "--local",
        &review_arg,
        "--peer",
        &server.url,
        "--token-file",
        &token_arg,
    ]);
    assert_eq!(
        query::<String>(
            &telescope,
            "SELECT rejectreason FROM acquiredimage WHERE guid = 'image-two'"
        ),
        "sent over http"
    );
    drop(server);
}

#[test]
fn a_remote_sync_without_a_key_stops_before_it_reaches_the_peer() {
    let directory = TempDir::new().unwrap();
    let review = directory.path().join("review.sqlite");
    scheduler_db(&review, REVIEW_ROWS);

    let output = Command::new(env!("CARGO_BIN_EXE_psf-guard"))
        .args([
            "sync",
            "remote",
            "--direction",
            "pull",
            "--local",
            &path_arg(&review),
            "--peer",
            "http://127.0.0.1:9",
        ])
        // The variable is a supported way to supply the key, so an inherited
        // one would make this pass for the wrong reason.
        .env_remove("PSF_GUARD_SYNC_TOKEN")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("--token-file"), "{message}");
}

#[test]
fn a_peer_url_without_a_scheme_is_refused_before_any_request() {
    let directory = TempDir::new().unwrap();
    let review = directory.path().join("review.sqlite");
    scheduler_db(&review, REVIEW_ROWS);

    let output = run(&[
        "sync",
        "remote",
        "--direction",
        "pull",
        "--local",
        &path_arg(&review),
        "--peer",
        "telescope.local:3000",
        "--token",
        TELESCOPE_TOKEN,
    ]);
    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("http://"), "{message}");
}

fn count(path: &Path, table: &str) -> i64 {
    query(path, &format!("SELECT count(*) FROM {table}"))
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A real `psf-guard server` on a free port, opened for remote sync by a
/// config file — the only way a headless instance grants it.
struct Server {
    child: Child,
    url: String,
}

impl Server {
    fn start(directory: &Path, database: &Path) -> Self {
        // Bind port 0 to have the OS name a free one, then release it. A racing
        // process could still take it; the readiness wait below turns that into
        // a clear timeout rather than a confusing refusal later.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let images = directory.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let registry = directory.join("registry.json");
        std::fs::write(
            &registry,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 2,
                "databases": [{
                    "id": "telescope",
                    "name": "Telescope",
                    "db_path": database,
                    "image_dirs": [images],
                }],
            }))
            .unwrap(),
        )
        .unwrap();
        let config = directory.join("psf-guard.toml");
        let mut file = std::fs::File::create(&config).unwrap();
        writeln!(
            file,
            "[server]\n\n[cache]\n\n[[remote_sync]]\ndatabase = \"telescope\"\ntoken = \"{TELESCOPE_TOKEN}\"\n"
        )
        .unwrap();

        let child = Command::new(env!("CARGO_BIN_EXE_psf-guard"))
            .args([
                "server",
                "--port",
                &port.to_string(),
                "--host",
                "127.0.0.1",
                "--registry",
                &path_arg(&registry),
                "--cache-dir",
                &path_arg(&directory.join("cache")),
                "--config",
                &path_arg(&config),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("starting psf-guard server");

        let server = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
        };
        server.wait_until_ready();
        server
    }

    fn wait_until_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if TcpListener::bind(("127.0.0.1", self.port())).is_err() {
                // Something is listening. Give it a moment to finish binding
                // its routes before the first request.
                std::thread::sleep(Duration::from_millis(200));
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("psf-guard server did not start on {}", self.url);
    }

    fn port(&self) -> u16 {
        self.url.rsplit(':').next().unwrap().parse().unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // A leaked server would hold the temporary directory's database open
        // and keep a port bound for the rest of the run.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Paths built from a `TempDir` are absolute, so `psf-guard` must not consult
/// the registry for them at all — which is what lets these tests run without
/// touching the user's real config.
#[test]
fn a_path_argument_never_needs_the_registry() {
    let directory = TempDir::new().unwrap();
    let telescope = directory.path().join("telescope.sqlite");
    let review = directory.path().join("review.sqlite");
    scheduler_db(&telescope, TELESCOPE_ROWS);
    scheduler_db(&review, REVIEW_ROWS);

    let output = Command::new(env!("CARGO_BIN_EXE_psf-guard"))
        .args([
            "sync",
            "pull",
            "--from",
            &path_arg(&telescope),
            "--to",
            &path_arg(&review),
            "--dry-run",
            // A registry path that does not exist: reaching for it would fail.
            "--registry",
            &path_arg(&directory.path().join("nowhere/registry.json")),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Guard against the binary silently losing the subcommand.
#[test]
fn the_remote_command_is_reachable_from_the_help() {
    let help = run_ok(&["sync", "--help"]);
    assert!(help.contains("remote"), "{help}");
}
