//! `psf-guard rc-astro license`: RC-Astro's account and license state on
//! this computer, for someone at a worker machine who wants to see what
//! the worker sees without going through the server.

use anyhow::{bail, Result};
use std::path::Path;

use crate::cli::RcAstroAction;
use crate::remote_work::rc_astro::{self, LicenseCheck};

pub fn run(action: RcAstroAction) -> Result<()> {
    match action {
        RcAstroAction::License { json, rc_astro } => {
            let Some(cli) = rc_astro::locate(rc_astro.as_deref().map(Path::new)) else {
                bail!(
                    "rc-astro was not found here (looked in {}). Give its path with --rc-astro",
                    rc_astro::searched_places()
                );
            };
            let check = rc_astro::check_license(&cli, rc_astro::LICENSE_CHECK_LIMIT, &|| false);
            if json {
                println!("{}", serde_json::to_string_pretty(&check)?);
            } else {
                print!("{}", describe(&check));
            }
            // A script can tell a clean report from anything else.
            if !check.succeeded() {
                std::process::exit(exit_status(&check));
            }
            Ok(())
        }
    }
}

/// The command's exit status: 0 for a report with no error, 77 when
/// RC-Astro reported an error, as rc-astro itself exits for a licensing
/// refusal, and 1 when PSF Guard got no answer from rc-astro.
pub fn exit_status(check: &LicenseCheck) -> i32 {
    if check.succeeded() {
        0
    } else if check.failure.is_none() && check.error.is_some() {
        77
    } else {
        1
    }
}

/// The check as a person reads it.
pub fn describe(check: &LicenseCheck) -> String {
    let mut lines = vec![format!(
        "rc-astro: {}{}",
        check.executable,
        check
            .cli_version
            .as_deref()
            .map(|version| format!(" (version {version})"))
            .unwrap_or_default()
    )];
    if let Some(report) = &check.report {
        lines.push(format!("Computer: {}", report.computer));
        lines.push(match &report.account_email {
            Some(email) => format!("RC-Astro account: {email}"),
            None => format!(
                "This computer is not registered to an RC-Astro account{}",
                report
                    .activation_url
                    .as_deref()
                    .map(|url| format!(". Get an activation code at {url}"))
                    .unwrap_or_default()
            ),
        });
        for product in &report.products {
            lines.push(format!("  {}: {}", product.name, product.message));
        }
        if let Some(attention) = &report.attention {
            lines.push(attention.message.clone());
            if let Some(command) = &attention.command {
                lines.push(format!("Run in a terminal: {command}"));
            }
        }
    }
    // RC-Astro's words are marked as RC-Astro's; PSF Guard's own failure
    // to get an answer is said plainly.
    if let Some(error) = &check.error {
        lines.push(format!("RC-Astro reported: {error}"));
    }
    if let Some(failure) = &check.failure {
        lines.push(failure.clone());
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_reads_as_rc_astro_wrote_it() {
        let check = rc_astro::parse_license_stream(
            "/opt/rc-astro/rc-astro",
            concat!(
                r#"{"event":"info","topic":"version","cliVersion":"2.6.10","schemaVersion":6}"#,
                "\n",
                r#"{"event":"info","topic":"licenseReport","operation":"check","computer":"svc@worker","activationUrl":"https://www.rc-astro.com/account","licenseStatus":[{"product":"nxt","name":"NoiseXTerminator","status":"required","message":"Not activated"}],"attention":{"reason":"unregistered","message":"This computer is not registered to an RC-Astro account","command":"rc-astro license <code>"}}"#,
                "\n",
                r#"{"event":"error","message":"No license"}"#,
            ),
        );
        assert_eq!(
            describe(&check),
            "rc-astro: /opt/rc-astro/rc-astro (version 2.6.10)\n\
             Computer: svc@worker\n\
             This computer is not registered to an RC-Astro account. Get an activation code at https://www.rc-astro.com/account\n  \
             NoiseXTerminator: Not activated\n\
             This computer is not registered to an RC-Astro account\n\
             Run in a terminal: rc-astro license <code>\n\
             RC-Astro reported: No license\n"
        );
    }

    #[test]
    fn the_exit_status_tells_a_clean_report_from_a_refusal_and_from_no_answer() {
        let report = Some(rc_astro::LicenseReport::default());
        let clean = LicenseCheck {
            report: report.clone(),
            ..Default::default()
        };
        assert_eq!(exit_status(&clean), 0);
        // A refusal carries its report and RC-Astro's error.
        let refused = LicenseCheck {
            report,
            error: Some("One product refused".into()),
            ..Default::default()
        };
        assert_eq!(exit_status(&refused), 77);
        // Offline: RC-Astro's error and no report.
        let offline = LicenseCheck {
            error: Some("Could not reach the RC-Astro license server".into()),
            ..Default::default()
        };
        assert_eq!(exit_status(&offline), 77);
        // rc-astro could not be started, timed out or crashed: PSF Guard's
        // own words, and never "RC-Astro reported".
        let failed = LicenseCheck {
            executable: "/nowhere/rc-astro".into(),
            failure: Some("PSF Guard could not start /nowhere/rc-astro: not found.".into()),
            ..Default::default()
        };
        assert_eq!(exit_status(&failed), 1);
        let said = describe(&failed);
        assert!(!said.contains("RC-Astro reported"), "{said}");
        assert!(said.ends_with("PSF Guard could not start /nowhere/rc-astro: not found.\n"));
    }
}
