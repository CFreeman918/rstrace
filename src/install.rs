//! `--install` / `--uninstall`: per-user desktop integration on Linux.
//!
//! Wayland compositors don't let an app set its own window icon; they look it
//! up from an installed `.desktop` entry whose name matches the app id
//! ("rstrace"). This installs, for the current user only (no sudo):
//!   ~/.local/bin/rstrace                                  (the program)
//!   ~/.local/share/applications/rstrace.desktop           (menu entry)
//!   ~/.local/share/icons/hicolor/{256x256,512x512}/apps/rstrace.png

use std::path::{Path, PathBuf};

const ICON_256: &[u8] = include_bytes!("../assets/icon.png");
const ICON_512: &[u8] = include_bytes!("../assets/icon-512.png");

struct Paths {
    bin: PathBuf,
    desktop: PathBuf,
    icons: Vec<(PathBuf, &'static [u8])>,
    data_home: PathBuf,
}

fn paths() -> Result<Paths, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    let icon = |size: &str| {
        data_home
            .join("icons/hicolor")
            .join(size)
            .join("apps/rstrace.png")
    };
    Ok(Paths {
        bin: home.join(".local/bin/rstrace"),
        desktop: data_home.join("applications/rstrace.desktop"),
        icons: vec![(icon("256x256"), ICON_256), (icon("512x512"), ICON_512)],
        data_home,
    })
}

fn desktop_entry(exec: &Path) -> String {
    // Quote the path per the Desktop Entry spec in case it contains spaces.
    let exec = exec
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=rstrace\n\
         GenericName=Photo to vector tracer\n\
         Comment=Trace photos taken in a Shaper Trace style frame to real-scale SVG or DXF\n\
         Exec=\"{exec}\" --gui %f\n\
         Icon=rstrace\n\
         Terminal=false\n\
         Categories=Graphics;2DGraphics;VectorGraphics;Engineering;\n\
         MimeType=image/jpeg;image/png;image/webp;image/tiff;\n\
         StartupWMClass=rstrace\n"
    )
}

fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, data).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

/// Best-effort cache refreshes so menus pick the entry up immediately.
fn refresh_caches(p: &Paths) {
    let run = |cmd: &str, args: &[&std::ffi::OsStr]| {
        let _ = std::process::Command::new(cmd)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    };
    run(
        "update-desktop-database",
        &[p.data_home.join("applications").as_os_str()],
    );
    run(
        "gtk-update-icon-cache",
        &[
            "-f".as_ref(),
            "-t".as_ref(),
            p.data_home.join("icons/hicolor").as_os_str(),
        ],
    );
}

pub fn install() -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err(
            "--install is for Linux. On Windows the .exe already carries its icon; \
                    on macOS use the rstrace.app bundle from the CI build."
                .into(),
        );
    }
    let p = paths()?;
    let exe = std::env::current_exe().map_err(|e| format!("Cannot locate this program: {e}"))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    if exe != p.bin {
        if let Some(dir) = p.bin.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        }
        // Copy to a temp name then rename, so a running copy isn't truncated.
        let tmp = p.bin.with_extension("new");
        std::fs::copy(&exe, &tmp).map_err(|e| format!("Cannot copy to {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p.bin)
            .map_err(|e| format!("Cannot install {}: {e}", p.bin.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p.bin, std::fs::Permissions::from_mode(0o755));
        }
    }
    for (path, data) in &p.icons {
        write(path, data)?;
    }
    write(&p.desktop, desktop_entry(&p.bin).as_bytes())?;
    refresh_caches(&p);

    println!("Installed rstrace for this user:");
    println!("  program   {}", p.bin.display());
    println!("  menu      {}", p.desktop.display());
    for (path, _) in &p.icons {
        println!("  icon      {}", path.display());
    }
    let on_path = std::env::var_os("PATH")
        .map(|v| std::env::split_paths(&v).any(|d| d == p.bin.parent().unwrap_or(Path::new(""))))
        .unwrap_or(false);
    if !on_path {
        println!(
            "\nNote: {} is not on your PATH, so typing `rstrace` in a terminal won't find it.\n\
             The menu entry works regardless. To fix the terminal, add it to PATH in your shell profile.",
            p.bin.parent().unwrap().display()
        );
    }
    println!("\nLaunch rstrace from your applications menu. If the icon doesn't appear right away, log out and back in.");
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let p = paths()?;
    let mut removed = 0;
    for path in std::iter::once(&p.desktop)
        .chain(p.icons.iter().map(|(p, _)| p))
        .chain(std::iter::once(&p.bin))
    {
        match std::fs::remove_file(path) {
            Ok(()) => {
                println!("removed {}", path.display());
                removed += 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot remove {}: {e}", path.display())),
        }
    }
    refresh_caches(&p);
    if removed == 0 {
        println!("Nothing to remove: rstrace was not installed with --install.");
    }
    Ok(())
}
