//! Locate Node runtime (local compatible service is a Node script).

use std::path::PathBuf;

/// First candidate Node (no version check); to run local service use [`node_candidates`] and test each.
pub fn locate_node() -> Option<PathBuf> {
    node_candidates().into_iter().next()
}

/// All Node candidates, priority order:
/// `CLAUDE_LOCAL_NODE` (or `CODEX_PLUS_NODE`) → PATH and common install dirs → other Apps' bundled Node.
///
/// System Node ranks before other Apps' bundled Node (#17): borrowing binary from ChatGPT / Codex package
/// shows as "ChatGPT connecting" in firewall, and breaks when that App updates or uninstalls.
/// Fallback only when user has no Node or version is too old.
pub fn node_candidates() -> Vec<PathBuf> {
    let configured = std::env::var_os("CLAUDE_LOCAL_NODE")
        .or_else(|| std::env::var_os("CODEX_PLUS_NODE"))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file());
    let bundled = bundled_nodes().into_iter().filter(|path| path.is_file()).collect();
    ordered_candidates(configured, program_candidates("node"), bundled)
}

fn ordered_candidates(
    configured: Option<PathBuf>,
    system: Vec<PathBuf>,
    bundled: Vec<PathBuf>,
) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    configured
        .into_iter()
        .chain(system)
        .chain(bundled)
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

/// Other Apps' bundled Node (Codex / ChatGPT cua_node).
fn bundled_nodes() -> Vec<PathBuf> {
    #[allow(unused_mut)]
    let mut paths = Vec::new();
    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from(
            "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node",
        ));
        paths.push(PathBuf::from(
            "/Applications/Codex.app/Contents/Resources/cua_node/bin/node",
        ));
        if let Some(base) = directories::BaseDirs::new() {
            paths.push(
                base.home_dir()
                    .join("Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node"),
            );
        }
    }
    paths
}

/// Find program in PATH and common install dirs. GUI-launched apps don't get shell PATH, add common locations.
pub fn locate_program(name: &str) -> Option<PathBuf> {
    program_candidates(name).into_iter().next()
}

/// All programs with this name in PATH and common install dirs, in search order.
fn program_candidates(name: &str) -> Vec<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(base) = directories::BaseDirs::new() {
        for extra in [".local/bin", ".volta/bin", ".npm-global/bin", ".bun/bin"] {
            dirs.push(base.home_dir().join(extra));
        }
    }
    if cfg!(unix) {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/usr/bin"));
    }
    dirs.into_iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .filter(|path| path.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_node_comes_before_nodes_bundled_in_other_apps() {
        let configured = PathBuf::from("/custom/node");
        let system = PathBuf::from("/usr/local/bin/node");
        let bundled = PathBuf::from("/Applications/Codex.app/Contents/Resources/cua_node/bin/node");
        assert_eq!(
            ordered_candidates(None, vec![system.clone()], vec![bundled.clone()]),
            vec![system.clone(), bundled.clone()],
        );
        // Manually specified always first; same path not checked twice.
        assert_eq!(
            ordered_candidates(
                Some(configured.clone()),
                vec![system.clone(), configured.clone()],
                vec![bundled.clone()],
            ),
            vec![configured, system, bundled.clone()],
        );
        // Fallback to other Apps' Node only when no system Node installed.
        assert_eq!(ordered_candidates(None, vec![], vec![bundled.clone()]), vec![bundled]);
    }
}