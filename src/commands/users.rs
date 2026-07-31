use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::{
    auth_registry::{AuthRegistry, AuthUserRecord},
    cli::UserCommand,
    db_registry::DbRegistry,
};

pub fn manage_users(action: UserCommand) -> Result<()> {
    let registry_argument = match &action {
        UserCommand::List { registry }
        | UserCommand::Add { registry, .. }
        | UserCommand::Remove { registry, .. } => registry,
    };
    let database_registry_path = match registry_argument {
        Some(path) => PathBuf::from(path),
        None => DbRegistry::default_path().context("resolving default registry path")?,
    };
    let auth_path = AuthRegistry::path_for_database_registry(&database_registry_path);
    let mut registry = AuthRegistry::load(&auth_path)?;

    match action {
        UserCommand::List { .. } => {
            if registry.users.is_empty() {
                println!("No browser users in {}", auth_path.display());
            } else {
                println!("{:<32} {:<12} EMAIL", "USERNAME", "ROLE");
                for user in &registry.users {
                    println!(
                        "{:<32} {:<12} {}",
                        user.username,
                        user.role,
                        user.email.as_deref().unwrap_or("")
                    );
                }
                println!("\nAuth registry: {}", auth_path.display());
            }
        }
        UserCommand::Add {
            username,
            role,
            email,
            password_file,
            replace,
            ..
        } => {
            let password = read_new_password(password_file.as_deref())?;
            let previous_email = if replace && email.is_none() {
                registry
                    .users
                    .iter()
                    .find(|user| user.username == username)
                    .and_then(|user| user.email.clone())
            } else {
                None
            };
            let user = AuthUserRecord::new_with_email(
                &username,
                role.into(),
                email.as_deref().or(previous_email.as_deref()),
                &password,
            )?;
            registry.add(user, replace)?;
            registry.save(&auth_path)?;
            println!(
                "{} user '{}' in {}",
                if replace { "Saved" } else { "Added" },
                username,
                auth_path.display()
            );
            println!("Restart the server to apply this change.");
        }
        UserCommand::Remove {
            username,
            allow_empty,
            ..
        } => {
            if registry.users.len() == 1 && registry.users[0].username == username && !allow_empty {
                anyhow::bail!("'{username}' is the last user; pass --allow-empty to remove it");
            }
            registry.remove(&username)?;
            registry.save(&auth_path)?;
            println!("Removed user '{}' from {}", username, auth_path.display());
            println!("Restart the server to apply this change.");
        }
    }
    Ok(())
}

fn read_new_password(password_file: Option<&str>) -> Result<String> {
    if let Some(path) = password_file {
        return Ok(std::fs::read_to_string(path)
            .with_context(|| format!("reading password from {path}"))?
            .trim_end_matches(['\r', '\n'])
            .to_string());
    }
    let password = rpassword::prompt_password("Password: ").context("reading password")?;
    let confirmation = rpassword::prompt_password("Confirm password: ")
        .context("reading password confirmation")?;
    if password != confirmation {
        anyhow::bail!("passwords do not match");
    }
    Ok(password)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_files_keep_spaces_but_drop_the_final_newline() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("password");
        std::fs::write(&path, "  a password with spaces  \r\n").unwrap();

        assert_eq!(
            read_new_password(path.to_str()).unwrap(),
            "  a password with spaces  "
        );
    }
}
