//! Let KWin own the main window's position on Plasma (X11 and Wayland).
use glib::{KeyFile, KeyFileFlags};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const RULE_ID: &str = "kimaitray-popup-position";
const RULE: &[(&str, &str)] = &[
    ("Description", "KimaiTray - remember main window position"),
    ("wmclass", "(?i)^kimaitray$"),
    ("wmclassmatch", "3"),
    ("wmclasscomplete", "false"),
    ("title", "KimaiTray"),
    ("titlematch", "1"),
    ("positionrule", "4"),
];
static REMEMBERS_POSITION: AtomicBool = AtomicBool::new(false);
static RELOAD_PENDING: AtomicBool = AtomicBool::new(false);

pub fn remembers_position() -> bool {
    REMEMBERS_POSITION.load(Ordering::Relaxed)
}

fn is_plasma(desktop: &str) -> bool {
    desktop.split(':').any(|name| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "kde" | "plasma" | "plasmawayland"
        )
    })
}

pub fn initialize() {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("XDG_SESSION_DESKTOP"))
        .unwrap_or_default();
    if !is_plasma(&desktop) {
        return;
    }
    // Normally finishes before the first show. Autostart can race KWin's bus
    // registration; retry off the UI thread without holding up the application.
    if let Err(error) = initialize_once() {
        log::warn!("KDE position remembering: {error}");
        std::thread::spawn(|| {
            for delay in [1, 2] {
                std::thread::sleep(Duration::from_secs(delay));
                match initialize_once() {
                    Ok(()) => return,
                    Err(error) => log::warn!("KDE position remembering: {error}"),
                }
            }
        });
    }
}

fn initialize_once() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    kwin_call("org.freedesktop.DBus.Peer.Ping", deadline)?;
    let directory = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|path| Path::new(path).is_absolute())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or("Cannot find the user configuration directory")?;
    let path = directory.join("kwinrulesrc");
    // KConfig merges system configuration too. Do not replace an inherited
    // rule list using our view of just the user's file.
    let system_dirs = std::env::var_os("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".into());
    if std::env::split_paths(&system_dirs).any(|directory| directory.join("kwinrulesrc").is_file())
    {
        return Err("System-wide KWin rules found; leaving the configuration unchanged".into());
    }
    let writer = ["kwriteconfig6", "kwriteconfig5"]
        .into_iter()
        .find_map(find_program)
        .ok_or("kwriteconfig6/kwriteconfig5 is unavailable")?;
    let enabled = ensure_rule(&path, |group, key, value| {
        let mut command = Command::new(&writer);
        command.args(["--file"]).arg(&path);
        command.args(["--group", group, "--key", key, "--", value]);
        run(command, deadline)?;
        RELOAD_PENDING.store(true, Ordering::Relaxed);
        Ok(())
    })?;
    if enabled {
        // Reload only after writing. Reloading an unchanged rule can discard
        // positions that KWin has learned in memory but has not saved yet.
        if RELOAD_PENDING.load(Ordering::Relaxed) {
            kwin_call("org.kde.KWin.reconfigure", deadline)?;
            RELOAD_PENDING.store(false, Ordering::Relaxed);
        }
        REMEMBERS_POSITION.store(true, Ordering::Relaxed);
        log::info!("KWin remembers the KimaiTray main window position");
    }
    Ok(())
}

fn find_program(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|directory| {
        let path = directory.join(name);
        path.is_file().then_some(path)
    })
}

fn kwin_call(method: &str, deadline: Instant) -> Result<(), String> {
    let mut command = Command::new("gdbus");
    command.args([
        "call",
        "--session",
        "--dest",
        "org.kde.KWin",
        "--object-path",
        "/KWin",
        "--method",
        method,
    ]);
    run(command, deadline)
}

fn run(mut command: Command, deadline: Instant) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    if Instant::now() >= deadline {
        return Err("KDE setup timed out".into());
    }
    // AppImage's bundled Qt libraries must not be loaded by host KDE tools.
    if std::env::var_os("APPIMAGE").is_some() {
        command
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("QT_PLUGIN_PATH");
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{program}: {error}"))?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} exited with {status}")),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(error) => error.to_string(),
                    _ => format!("{program} timed out"),
                });
            }
        }
    }
}

fn read(path: &Path) -> Result<KeyFile, String> {
    let data = match std::fs::read_to_string(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.to_string()),
    };
    let config = KeyFile::new();
    config
        .load_from_data(&data, KeyFileFlags::NONE)
        .map_err(|error| error.to_string())?;
    Ok(config)
}

fn value(config: &KeyFile, group: &str, key: &str) -> String {
    config
        .value(group, key)
        .map(|value| value.to_string())
        .unwrap_or_default()
}

fn rule_ids(config: &KeyFile) -> Result<Vec<String>, String> {
    if config.has_key("General", "order").unwrap_or(false) {
        return Err("Unrecognised KWin rule ordering format; leaving it unchanged".into());
    }
    let rules = value(config, "General", "rules");
    let ids: Vec<String> = if rules.is_empty() {
        let count = value(config, "General", "count");
        let count = if count.is_empty() {
            0
        } else {
            count.parse::<usize>().map_err(|error| error.to_string())?
        };
        if count > 10000 {
            return Err("Invalid KWin rule count".into());
        }
        (1..=count).map(|id| id.to_string()).collect()
    } else {
        rules.split(',').map(str::to_owned).collect()
    };
    // KConfig's escaped list syntax is not GLib's list syntax. Only extend a
    // list we can round-trip without changing any existing identifiers.
    if ids.iter().any(|id| {
        id.is_empty()
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    }) {
        return Err("Unrecognised KWin rule identifiers; leaving them unchanged".into());
    }
    Ok(ids)
}

fn existing_policy(config: &KeyFile, ids: &[String]) -> Option<bool> {
    for id in ids {
        let class = value(config, id, "wmclass");
        let class_match = value(config, id, "wmclassmatch");
        let matches_class = (class_match == "1" && class.eq_ignore_ascii_case("kimaitray"))
            || (class_match == "3" && class == "(?i)^kimaitray$");
        if matches_class
            && value(config, id, "title") == "KimaiTray"
            && value(config, id, "titlematch") == "1"
        {
            let position = value(config, id, "positionrule");
            if id != RULE_ID && matches!(position.as_str(), "" | "0") {
                continue;
            }
            // A user's customised/disabled rule is not ours to repair.
            return Some(
                position == "4"
                    // Exact class matches can cover only one of the native
                    // Wayland/XWayland identities; preserve without assuming
                    // they manage this window in both sessions.
                    && class_match == "3"
                    && !matches!(value(config, id, "enabled").as_str(), "false" | "0")
                    && matches!(
                        value(config, id, "wmclasscomplete").as_str(),
                        "" | "false" | "0"
                    )
                    && config.keys(id).is_ok_and(|keys| keys.iter().all(|key| {
                        !key.ends_with("match") || matches!(key.as_str(), "wmclassmatch" | "titlematch")
                            || matches!(value(config, id, key).as_str(), "" | "0")
                    }))
                    && value(config, id, "types").parse::<u32>().unwrap_or(u32::MAX) & 1 != 0,
            );
        }
    }
    None
}

fn ensure_rule(
    path: &Path,
    mut write: impl FnMut(&str, &str, &str) -> Result<(), String>,
) -> Result<bool, String> {
    let config = read(path)?;
    let ids = rule_ids(&config)?;
    if let Some(enabled) = existing_policy(&config, &ids) {
        return Ok(enabled);
    }
    // Do not overwrite a user-edited or partially written group with our ID.
    if config.has_group(RULE_ID) || ids.iter().any(|id| id == RULE_ID) {
        return Ok(false);
    }
    for &(key, value) in RULE {
        write(RULE_ID, key, value)?;
    }
    // Register only the complete group, using a fresh list so intervening
    // changes to other rules are retained. KConfig handles individual writes.
    let config = read(path)?;
    if RULE
        .iter()
        .any(|&(key, expected)| value(&config, RULE_ID, key) != expected)
    {
        return Err("KWin rule write could not be verified".into());
    }
    let mut ids = rule_ids(&config)?;
    if !ids.iter().any(|id| id == RULE_ID) {
        ids.push(RULE_ID.into());
    }
    write("General", "rules", &ids.join(","))?;
    write("General", "count", &ids.len().to_string())?;
    if rule_ids(&read(path)?)? != ids {
        return Err("KWin rule registration could not be verified".into());
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(path: &Path, group: &str, key: &str, value: &str) -> Result<(), String> {
        let config = read(path)?;
        config.set_value(group, key, value);
        std::fs::write(path, config.to_data()).map_err(|error| error.to_string())
    }

    fn install(path: &Path) -> Result<bool, String> {
        ensure_rule(path, |group, key, value| {
            write_fixture(path, group, key, value)
        })
    }

    #[test]
    fn only_plasma_sessions_enable_setup() {
        for desktop in ["KDE", "kde", "Plasma", "plasmawayland", "KDE:Plasma"] {
            assert!(is_plasma(desktop), "{desktop}");
        }
        for desktop in ["", "GNOME", "ubuntu:GNOME", "XFCE", "not-kde"] {
            assert!(!is_plasma(desktop), "{desktop}");
        }
    }

    #[test]
    fn installs_once_without_seeding_position_or_size() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kwinrulesrc");
        assert!(install(&path).unwrap());
        let config = read(&path).unwrap();
        assert_eq!(rule_ids(&config).unwrap(), [RULE_ID]);
        assert!(!config.has_key(RULE_ID, "position").unwrap());
        assert!(!config.has_key(RULE_ID, "size").unwrap());
        write_fixture(&path, RULE_ID, "position", "3395,393").unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(ensure_rule(&path, |_, _, _| panic!(
            "existing rule must not be rewritten"
        ))
        .unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn preserves_manual_rule_under_another_identifier() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kwinrulesrc");
        for &(key, value) in RULE {
            write_fixture(&path, "user-rule", key, value).unwrap();
        }
        write_fixture(&path, "General", "rules", "user-rule").unwrap();
        write_fixture(&path, "user-rule", "position", "-500,120").unwrap();
        assert!(ensure_rule(&path, |_, _, _| panic!("manual rule must be preserved")).unwrap());
    }

    #[test]
    fn preserves_customised_disabled_and_unregistered_rules() {
        for (key, value) in [
            ("positionrule", "2"),
            ("enabled", "false"),
            ("wmclass", "custom"),
            ("windowrolematch", "1"),
            ("types", "2"),
            ("General", ""),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("kwinrulesrc");
            install(&path).unwrap();
            if key == "General" {
                write_fixture(&path, "General", "rules", "other").unwrap();
            } else {
                write_fixture(&path, RULE_ID, key, value).unwrap();
            }
            assert!(
                !ensure_rule(&path, |_, _, _| panic!("customisation must be preserved")).unwrap()
            );
        }
    }

    #[test]
    fn preserves_legacy_order_and_concurrent_additions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kwinrulesrc");
        std::fs::write(
            &path,
            "[General]\ncount=2\n[1]\nDescription=First\n[2]\nDescription=Second\n",
        )
        .unwrap();
        assert!(ensure_rule(&path, |group, key, value| {
            write_fixture(&path, group, key, value)?;
            if key == "positionrule" {
                write_fixture(&path, "General", "rules", "1,2,new-user-rule")?;
            }
            Ok(())
        })
        .unwrap());
        let config = read(&path).unwrap();
        assert_eq!(
            rule_ids(&config).unwrap(),
            ["1", "2", "new-user-rule", RULE_ID]
        );
        assert_eq!(value(&config, "1", "Description"), "First");
    }

    #[test]
    fn refuses_unrecognised_configuration_before_writing() {
        for data in [
            "[General]\nrules=escaped\\,identifier\n",
            "[General]\norder=one,two\n",
            "not an ini file",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("kwinrulesrc");
            std::fs::write(&path, data).unwrap();
            assert!(
                ensure_rule(&path, |_, _, _| panic!("unsafe config must not be changed")).is_err()
            );
        }
    }

    #[test]
    fn does_not_register_incomplete_or_failed_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kwinrulesrc");
        assert!(ensure_rule(&path, |_, _, _| Err("read only".into())).is_err());
        assert!(ensure_rule(&path, |_, _, _| Ok(())).is_err());
        assert!(rule_ids(&read(&path).unwrap()).unwrap().is_empty());
    }

    #[test]
    fn kills_and_reaps_a_stalled_helper() {
        let mut command = Command::new("sleep");
        command.arg("10");
        let started = Instant::now();
        assert!(run(command, started + Duration::from_millis(30)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    #[ignore = "requires KDE's kwriteconfig6 utility; writes only to a temporary directory"]
    fn native_kconfig_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kwinrulesrc");
        std::fs::write(&path, "[General]\ncount=1\nrules=other\n[other]\nDescription=User rule\nposition=50,70\npositionrule=4\n").unwrap();
        assert!(ensure_rule(&path, |group, key, value| {
            let mut command = Command::new("kwriteconfig6");
            command
                .arg("--file")
                .arg(&path)
                .args(["--group", group, "--key", key, "--", value]);
            run(command, Instant::now() + Duration::from_secs(2))
        })
        .unwrap());
        let config = read(&path).unwrap();
        assert_eq!(rule_ids(&config).unwrap(), ["other", RULE_ID]);
        assert_eq!(value(&config, "other", "position"), "50,70");
        assert!(ensure_rule(&path, |_, _, _| panic!("second launch must not write")).unwrap());
    }
}
