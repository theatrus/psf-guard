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
            let check = rc_astro::check_license(cli.executable(), rc_astro::LICENSE_CHECK_LIMIT);
            if json {
                println!("{}", serde_json::to_string_pretty(&check)?);
            } else {
                print!("{}", describe(&check));
            }
            Ok(())
        }
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
    if let Some(error) = &check.error {
        lines.push(format!("RC-Astro reported: {error}"));
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
}
