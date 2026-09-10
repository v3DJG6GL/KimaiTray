//! Let KWin own the main window's position on Plasma (X11 and Wayland).
use glib::{KeyFile, KeyFileFlags};
use gtk::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;
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
    let class = if gdk::Display::default()
        .is_some_and(|display| display.type_().name() == "GdkX11Display")
    {
        gdk::program_class()
            .map(|name| name.to_string())
            .unwrap_or_else(|| "Kimaitray".into())
    } else {
        glib::prgname()
            .map(|name| name.to_string())
            .unwrap_or_else(|| "kimaitray".into())
    };
    if let Err(error) = initialize_once(&class) {
        log::warn!("KDE position remembering: {error}");
        std::thread::spawn(move || {
            for delay in [1, 2] {
                std::thread::sleep(Duration::from_secs(delay));
                match initialize_once(&class) {
                    Ok(()) => return,
                    Err(error) => log::warn!("KDE position remembering: {error}"),
                }
            }
        });
    }
}

fn initialize_once(class: &str) -> Result<(), String> {
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
    let state = ensure_rule(&path, class, deadline, |stage, group, key, value| {
        let mut command = Command::new(&writer);
        command.arg("--file").arg(stage);
        command.args(["--group", group, "--key", key, "--", value]);
        crate::linux_command::run(command, deadline, false).map(|_| ())
    })?;
    if state == RuleState::Created {
        RELOAD_PENDING.store(true, Ordering::Relaxed);
    }
    if state != RuleState::Unmanaged {
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
    crate::linux_command::run(command, deadline, false).map(|_| ())
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

fn matches(pattern: &str, kind: &str, actual: &str) -> bool {
    match kind {
        "" | "0" => true,
        "1" => pattern == actual,
        "2" => actual.contains(pattern),
        "3" => {
            let (Ok(pattern), Ok(actual)) = (
                std::ffi::CString::new(pattern),
                std::ffi::CString::new(actual),
            ) else {
                return false;
            };
            // GLib and QRegularExpression use PCRE. Both C strings remain valid
            // for the call; g_regex_match_simple owns its temporary regex.
            unsafe { glib::ffi::g_regex_match_simple(pattern.as_ptr(), actual.as_ptr(), 0, 0) != 0 }
        }
        _ => false,
    }
}

fn existing_policy(config: &KeyFile, ids: &[String], class: &str) -> Option<bool> {
    for id in ids {
        if !matches(
            &value(config, id, "wmclass"),
            &value(config, id, "wmclassmatch"),
            class,
        ) || !matches(
            &value(config, id, "title"),
            &value(config, id, "titlematch"),
            "KimaiTray",
        ) {
            continue;
        }
        let position = value(config, id, "positionrule");
        if id != RULE_ID && matches!(position.as_str(), "" | "0") {
            continue;
        }
        // Keep user customisations. Additional matching constraints may exclude
        // this window, so do not assume those rules manage its position.
        return Some(
            position == "4"
                && !matches!(
                    value(config, id, "enabled").to_ascii_lowercase().as_str(),
                    "false" | "0" | "off" | "no"
                )
                && matches!(
                    value(config, id, "wmclasscomplete").as_str(),
                    "" | "false" | "0"
                )
                && config.keys(id).is_ok_and(|keys| {
                    keys.iter().all(|key| {
                        !key.ends_with("match")
                            || matches!(key.as_str(), "wmclassmatch" | "titlematch")
                            || matches!(value(config, id, key).as_str(), "" | "0")
                    })
                })
                && value(config, id, "types")
                    .parse::<u32>()
                    .unwrap_or(u32::MAX)
                    & 1
                    != 0,
        );
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
enum RuleState {
    Created,
    Existing,
    Unmanaged,
}

fn ensure_rule(
    path: &Path,
    class: &str,
    deadline: Instant,
    mut write: impl FnMut(&Path, &str, &str, &str) -> Result<(), String>,
) -> Result<RuleState, String> {
    let parent = path.parent().ok_or("No config directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    // Use KConfig's canonical lock path, including when kwinrulesrc is a symlink.
    let path = if path.exists() || path.is_symlink() {
        path.canonicalize().map_err(|e| e.to_string())?
    } else {
        parent
            .canonicalize()
            .map_err(|e| e.to_string())?
            .join(path.file_name().ok_or("No config filename")?)
    };
    let _lock = crate::kconfig_lock::ConfigLock::acquire(&path, deadline)?;
    let config = read(&path)?;
    let ids = rule_ids(&config)?;
    if let Some(enabled) = existing_policy(&config, &ids, class) {
        return Ok(if enabled {
            RuleState::Existing
        } else {
            RuleState::Unmanaged
        });
    }
    if config.has_group(RULE_ID) || ids.iter().any(|id| id == RULE_ID) {
        return Ok(RuleState::Unmanaged);
    }
    let stage_dir = tempfile::tempdir_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let stage = stage_dir.path().join("kwinrulesrc");
    if path.exists() {
        // Atomic replacement must not bypass permissions on the original file.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        std::fs::copy(&path, &stage).map_err(|e| e.to_string())?;
    } else {
        std::fs::write(&stage, "").map_err(|e| e.to_string())?;
    }
    for &(key, value) in RULE {
        write(&stage, RULE_ID, key, value)?;
    }
    let mut ids = ids;
    ids.push(RULE_ID.into());
    write(&stage, "General", "rules", &ids.join(","))?;
    write(&stage, "General", "count", &ids.len().to_string())?;
    let prepared = read(&stage)?;
    if rule_ids(&prepared)? != ids
        || RULE
            .iter()
            .any(|&(key, expected)| value(&prepared, RULE_ID, key) != expected)
    {
        return Err("KWin rule write could not be verified".into());
    }
    if Instant::now() >= deadline {
        return Err("KDE setup timed out".into());
    }
    // KConfig's writes replace the staged inode. Open that path again to sync
    // the finished file, then publish the entire transaction with one rename.
    std::fs::File::open(&stage)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&stage, &path).map_err(|e| e.to_string())?;
    Ok(RuleState::Created)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(data: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kwinrulesrc");
        std::fs::write(&path, data).unwrap();
        (directory, path)
    }

    fn write_fixture(path: &Path, group: &str, key: &str, value: &str) -> Result<(), String> {
        let config = read(path)?;
        config.set_value(group, key, value);
        std::fs::write(path, config.to_data()).map_err(|error| error.to_string())
    }

    fn install(path: &Path, class: &str) -> Result<RuleState, String> {
        ensure_rule(
            path,
            class,
            Instant::now() + Duration::from_secs(2),
            write_fixture,
        )
    }

    #[test]
    fn only_plasma_sessions_enable_setup() {
        for desktop in ["KDE", "kde", "Plasma", "plasmawayland", "KDE:Plasma"] {
            assert!(is_plasma(desktop));
        }
        for desktop in ["", "GNOME", "ubuntu:GNOME", "XFCE", "not-kde"] {
            assert!(!is_plasma(desktop));
        }
    }

    #[test]
    fn installs_once_without_seeding_position_or_size() {
        let (_directory, path) = fixture("");
        assert_eq!(install(&path, "kimaitray").unwrap(), RuleState::Created);
        let config = read(&path).unwrap();
        assert_eq!(rule_ids(&config).unwrap(), [RULE_ID]);
        assert!(!config.has_key(RULE_ID, "position").unwrap());
        assert!(!config.has_key(RULE_ID, "size").unwrap());
        write_fixture(&path, RULE_ID, "position", "3395,393").unwrap();
        let before = std::fs::read(&path).unwrap();
        assert_eq!(
            ensure_rule(
                &path,
                "kimaitray",
                Instant::now() + Duration::from_secs(2),
                |_, _, _, _| panic!("must not write")
            )
            .unwrap(),
            RuleState::Existing
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn recognises_exact_and_equivalent_regex_manual_rules_for_current_identity() {
        for class in ["kimaitray", "Kimaitray"] {
            for (pattern, kind) in [
                (class, "1"),
                ("^[Kk]imaitray$", "3"),
                ("(?i)^kimaitray$", "3"),
            ] {
                let (_directory, path) = fixture("[General]\nrules=manual\n");
                for &(key, value) in RULE {
                    write_fixture(&path, "manual", key, value).unwrap();
                }
                write_fixture(&path, "manual", "wmclass", pattern).unwrap();
                write_fixture(&path, "manual", "wmclassmatch", kind).unwrap();
                write_fixture(&path, "manual", "position", "-500,120").unwrap();
                let before = std::fs::read(&path).unwrap();
                assert_eq!(install(&path, class).unwrap(), RuleState::Existing);
                assert_eq!(std::fs::read(&path).unwrap(), before);
            }
        }
        assert!(!matches("Kimaitray", "1", "kimaitray"));
        assert!(!matches("KimaiTray", "1", "KimaiTray — Settings"));
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
            let (_directory, path) = fixture("");
            install(&path, "kimaitray").unwrap();
            if key == "General" {
                write_fixture(&path, "General", "rules", "other").unwrap();
            } else {
                write_fixture(&path, RULE_ID, key, value).unwrap();
            }
            let before = std::fs::read(&path).unwrap();
            assert_eq!(install(&path, "kimaitray").unwrap(), RuleState::Unmanaged);
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn reads_concurrent_updates_only_after_acquiring_the_config_lock() {
        let (_directory, path) =
            fixture("[General]\ncount=2\n[1]\nDescription=First\n[2]\nDescription=Second\n");
        let lock = crate::kconfig_lock::ConfigLock::acquire(
            &path,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        let worker_path = path.clone();
        let worker = std::thread::spawn(move || install(&worker_path, "kimaitray"));
        // Model the editor committing while it owns KDE's lock. The app must
        // wait and then read this version, not append to an earlier snapshot.
        write_fixture(&path, "General", "rules", "1,2,new-user-rule").unwrap();
        drop(lock);
        assert_eq!(worker.join().unwrap().unwrap(), RuleState::Created);
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
            let (_directory, path) = fixture(data);
            assert!(ensure_rule(
                &path,
                "kimaitray",
                Instant::now() + Duration::from_secs(2),
                |_, _, _, _| panic!("must not write")
            )
            .is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), data);
        }
    }

    #[test]
    fn failed_writes_leave_no_partial_rule_and_retry_succeeds() {
        for failure in [1, 2, 8, 9] {
            let (directory, path) = fixture("[General]\nrules=user\n[user]\nDescription=Keep me\n");
            let before = std::fs::read(&path).unwrap();
            let mut writes = 0;
            assert!(ensure_rule(
                &path,
                "kimaitray",
                Instant::now() + Duration::from_secs(2),
                |stage, group, key, value| {
                    writes += 1;
                    if writes == failure {
                        return Err("temporary write failure".into());
                    }
                    write_fixture(stage, group, key, value)
                }
            )
            .is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
            assert_eq!(install(&path, "kimaitray").unwrap(), RuleState::Created);
        }
    }

    #[test]
    fn rejects_unverified_writes_and_keeps_symlink_targets() {
        let (directory, path) = fixture("");
        assert!(ensure_rule(
            &path,
            "kimaitray",
            Instant::now() + Duration::from_secs(2),
            |_, _, _, _| Ok(())
        )
        .is_err());
        let link = directory.path().join("linked-rules");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(install(&link, "kimaitray").unwrap(), RuleState::Created);
        assert!(link.is_symlink());
        assert_eq!(rule_ids(&read(&path).unwrap()).unwrap(), [RULE_ID]);
    }

    #[test]
    #[ignore = "requires KDE's kwriteconfig6 utility; writes only to temporary configuration"]
    fn native_kconfig_round_trip() {
        let (_directory, path) = fixture("[General]\ncount=1\nrules=other\n[other]\nDescription=User rule\nwmclass=other\nwmclassmatch=1\nposition=50,70\npositionrule=4\n");
        assert_eq!(
            ensure_rule(
                &path,
                "kimaitray",
                Instant::now() + Duration::from_secs(2),
                |stage, group, key, value| {
                    let mut command = Command::new("kwriteconfig6");
                    command
                        .arg("--file")
                        .arg(stage)
                        .args(["--group", group, "--key", key, "--", value]);
                    crate::linux_command::run(
                        command,
                        Instant::now() + Duration::from_secs(2),
                        false,
                    )
                    .map(|_| ())
                }
            )
            .unwrap(),
            RuleState::Created
        );
        let config = read(&path).unwrap();
        assert_eq!(rule_ids(&config).unwrap(), ["other", RULE_ID]);
        assert_eq!(value(&config, "other", "position"), "50,70");
        assert_eq!(install(&path, "kimaitray").unwrap(), RuleState::Existing);
    }
}
