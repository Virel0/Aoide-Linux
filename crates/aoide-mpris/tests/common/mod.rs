//! A private `dbus-daemon` for the bus tests, shared by `tests/dbus.rs` and
//! `tests/tray.rs`. Each test starts its own from a minimal config with no
//! service directories, so nothing on the machine (a real portal, a real
//! screensaver, a real panel) is activated into it.

// Each test file uses its own part of this.
#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use aoide_mpris::Bus;
use zbus::Connection;

/// A `dbus-daemon` of our own, stopped on drop.
pub struct PrivateBus {
    daemon: Child,
    address: String,
    dir: PathBuf,
}

impl PrivateBus {
    pub fn start() -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "aoide-mpris-test-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                dir.display()
            ),
        )
        .unwrap();
        let mut daemon = Process::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon must be on the PATH to run these tests");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        assert!(!address.trim().is_empty(), "dbus-daemon printed no address");
        Self {
            daemon,
            address: address.trim().to_owned(),
            dir,
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn bus(&self) -> Bus {
        Bus::Address(self.address.clone())
    }

    pub async fn client(&self) -> Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
