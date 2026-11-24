//! Native "choose a folder" dialog without a GUI toolkit: osascript (macOS), PowerShell (Windows), zenity/kdialog (Linux).

use std::path::Path;
use std::process::Command;

fn command() -> Option<Command> {
    if cfg!(target_os = "macos") {
        let mut c = Command::new("osascript");
        c.args(["-e", "POSIX path of (choose folder with prompt \"Choose a folder to search\")"]);
        return Some(c);
    }
    if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-STA", "-Command", "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description = 'Choose a folder to search'; if ($d.ShowDialog() -eq 'OK') { Write-Output $d.SelectedPath }"]);
        return Some(c);
    }
    for (tool, args) in [
        ("zenity", &["--file-selection", "--directory", "--title=Choose a folder to search"][..]),
        ("kdialog", &["--getexistingdirectory"][..]),
    ] {
        if Command::new(tool).arg("--version").output().is_ok() {
            let mut c = Command::new(tool);
            c.args(args);
            return Some(c);
        }
    }
    None
}

/// Blocks until the user chooses or cancels. `None` if cancelled, unavailable, or not an existing directory.
pub(crate) fn pick_folder() -> Option<String> {
    let out = command()?.output().ok()?;
    let chosen = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!chosen.is_empty() && Path::new(&chosen).is_dir()).then_some(chosen)
}
