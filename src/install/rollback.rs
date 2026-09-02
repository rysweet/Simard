use std::path::Path;

use super::paths::InstallLayout;

pub fn print_guidance(layout: &InstallLayout, prior_binary_backup: Option<&Path>) {
    println!("Rollback planning:");
    match prior_binary_backup {
        Some(path) => println!("  Prior binary preserved at {}", path.display()),
        None => println!(
            "  No prior binary existed; future rollbacks will use {}",
            layout.backup_root.display()
        ),
    }
    println!(
        "  Before service restart, make a memory backup of {} if you need an operator-restorable state snapshot.",
        layout.simard_home.display()
    );
    println!(
        "  To roll back the binary, stop the services and copy the preserved binary from .install-backups back to {}.",
        layout.binary_path.display()
    );
}
