//! KConfig uses QLockFile: exclusive creation of `<canonical path>.lock`,
//! protected by flock on Linux. Match that protocol, not a separate app lock.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct ConfigLock {
    path: PathBuf,
    marker: tempfile::NamedTempFile,
}

fn same_file(file: &File, path: &Path) -> bool {
    file.metadata()
        .ok()
        .zip(path.metadata().ok())
        .is_some_and(|(a, b)| a.dev() == b.dev() && a.ino() == b.ino())
}

fn machine_file(path: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

impl ConfigLock {
    pub fn acquire(config: &Path, deadline: Instant) -> Result<Self, String> {
        let mut lock_name = config.as_os_str().to_owned();
        lock_name.push(".lock");
        let path = PathBuf::from(lock_name);
        let mut marker =
            tempfile::NamedTempFile::new_in(config.parent().ok_or("No config directory")?)
                .map_err(|e| e.to_string())?;
        // Prepare and flock the complete marker before publishing it. A crash
        // cannot leave an empty lock that looks like another live creator.
        marker.as_file().try_lock().map_err(|e| e.to_string())?;
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        writeln!(
            marker,
            "{}\n{}\n{}\n{}\n{}",
            std::process::id(),
            executable
                .file_name()
                .ok_or("No executable name")?
                .to_string_lossy(),
            machine_file("/proc/sys/kernel/hostname"),
            machine_file("/etc/machine-id"),
            machine_file("/proc/sys/kernel/random/boot_id")
        )
        .map_err(|e| e.to_string())?;
        loop {
            match std::fs::hard_link(marker.path(), &path) {
                Ok(()) => return Ok(Self { path, marker }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    remove_dead_lock(&path);
                    if Instant::now() >= deadline {
                        return Err("KWin configuration is locked".into());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

fn remove_dead_lock(path: &Path) {
    let Ok(mut file) = OpenOptions::new().read(true).write(true).open(path) else {
        return;
    };
    if file.try_lock().is_err() {
        return;
    }
    let mut text = String::new();
    if (&mut file).take(4096).read_to_string(&mut text).is_err() {
        return;
    }
    let fields: Vec<_> = text.lines().collect();
    let dead = fields
        .first()
        .and_then(|pid| pid.parse::<u32>().ok())
        .is_some_and(|pid| {
            let same_host = fields
                .get(2)
                .is_some_and(|host| *host == machine_file("/proc/sys/kernel/hostname"));
            let another_boot = fields.get(4).is_some_and(|boot| {
                !boot.is_empty() && *boot != machine_file("/proc/sys/kernel/random/boot_id")
            });
            same_host && pid > 0 && (another_boot || !Path::new(&format!("/proc/{pid}")).exists())
        });
    // QLockFile also permits removing an unlocked marker older than 30s.
    let expired = file
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|time| time.elapsed().ok())
        .is_some_and(|age| age > Duration::from_secs(30));
    if (dead || expired) && same_file(&file, path) {
        let _ = std::fs::remove_file(path);
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        if same_file(self.marker.as_file(), &self.path) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_other_holders_and_releases_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kwinrulesrc");
        let lock = ConfigLock::acquire(&path, Instant::now() + Duration::from_secs(1)).unwrap();
        assert!(ConfigLock::acquire(&path, Instant::now() + Duration::from_millis(30)).is_err());
        drop(lock);
        assert!(ConfigLock::acquire(&path, Instant::now() + Duration::from_secs(1)).is_ok());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn recovers_a_lock_left_by_a_dead_process() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kwinrulesrc");
        std::fs::write(
            directory.path().join("kwinrulesrc.lock"),
            format!(
                "4294967295\nkimaitray\n{}\n\n\n",
                machine_file("/proc/sys/kernel/hostname")
            ),
        )
        .unwrap();
        assert!(ConfigLock::acquire(&path, Instant::now() + Duration::from_secs(1)).is_ok());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    #[ignore = "requires KDE's kwriteconfig6 utility; uses temporary configuration only"]
    fn native_kconfig_writer_waits_for_our_transaction() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kwinrulesrc");
        std::fs::write(&path, "[General]\nrules=original\n").unwrap();
        let lock = ConfigLock::acquire(&path, Instant::now() + Duration::from_secs(1)).unwrap();
        let mut child = std::process::Command::new("kwriteconfig6")
            .arg("--file")
            .arg(&path)
            .args(["--group", "other", "--key", "Description", "User edit"])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let blocked = child.try_wait().unwrap().is_none();
        // Commit a different key while we own the lock. KConfig must merge its
        // pending edit with this version after obtaining the same lock.
        std::fs::write(
            &path,
            "[General]\nrules=original,kimaitray-popup-position\n",
        )
        .unwrap();
        drop(lock);
        let deadline = Instant::now() + Duration::from_secs(2);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = child.try_wait().unwrap();
        if status.is_none() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        assert!(blocked, "native KConfig bypassed the transaction lock");
        assert!(status.is_some_and(|status| status.success()));
        let data = std::fs::read_to_string(&path).unwrap();
        assert!(data.contains("rules=original,kimaitray-popup-position"));
        assert!(data.contains("Description=User edit"));
    }
}
