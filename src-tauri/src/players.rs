//! External media players we can hand a stream URL to.

use std::path::PathBuf;

/// (display name, executable names without `.exe`). Order = preference.
const CANDIDATES: &[(&str, &[&str])] = &[
    ("mpv", &["mpv"]),
    ("VLC", &["vlc"]),
    ("Celluloid", &["celluloid"]),
    ("Haruna", &["haruna"]),
    ("SMPlayer", &["smplayer"]),
    ("MPC-HC", &["mpc-hc64", "mpc-hc"]),
    ("PotPlayer", &["PotPlayerMini64", "PotPlayerMini"]),
];

/// Default install folders on Windows, which usually aren't on PATH.
fn extra_dirs() -> Vec<PathBuf> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(var).map(PathBuf::from) {
            for sub in ["VideoLAN\\VLC", "mpv", "MPC-HC", "DAUM\\PotPlayer", "SMPlayer"] {
                out.push(base.join(sub));
            }
        }
    }
    out
}

fn which(bin: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) { format!("{bin}.exe") } else { bin.to_string() };
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(extra_dirs())
        .map(|dir| dir.join(&file))
        .find(|p| p.is_file())
}

pub struct Player {
    pub name: &'static str,
    pub exe: PathBuf,
}

pub fn installed() -> Vec<Player> {
    CANDIDATES
        .iter()
        .filter_map(|(name, bins)| bins.iter().find_map(|b| which(b)).map(|exe| Player { name, exe }))
        .collect()
}
