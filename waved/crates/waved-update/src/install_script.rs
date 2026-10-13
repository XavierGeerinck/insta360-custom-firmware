//! `deploy/install.sh` against a fake Wave. A fake `adb` maps the Wave's
//! `/data`, `/tmp`, `/etc` and `/mnt` into a directory and runs `adb shell`
//! scripts with `sh`; fake busybox commands (`ps`, `killall`, `pidof`,
//! `start-stop-daemon`) log what they were asked. The watchdog and waved are
//! real `sleep` processes, so the script's `kill` reaches something. Nothing
//! here talks to a device.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

/// The fake `adb`: `push` copies, `shell` runs the script with `sh`, both with
/// the Wave's paths mapped into `$FAKE_WAVE`. `$FAKE_ADB_CORRUPT` names a
/// pushed path whose copy gets one byte too many.
const ADB: &str = r#"#!/bin/sh
printf 'adb %s\n' "$*" >> "$FAKE_WAVE/log"
on_wave() {
    printf '%s' "$1" | sed -E "s#(^|[^[:alnum:]_.-])/(data|tmp|etc|mnt)/#\1$FAKE_WAVE/\2/#g"
}
case "$1" in
    push)
        to=$(on_wave "$3")
        mkdir -p "$(dirname "$to")"
        cp "$2" "$to" || exit 1
        if [ -n "$FAKE_ADB_CORRUPT" ] && [ "$3" = "$FAKE_ADB_CORRUPT" ]; then
            printf x >> "$to"
        fi
        echo "$2: 1 file pushed."
        ;;
    shell)
        shift
        PATH="$FAKE_WAVE_BIN:$PATH" sh -c "$(on_wave "$*")"
        ;;
esac
"#;

/// Fake `llvm-objcopy`: "strips" by appending a marker the test can see.
const OBJCOPY: &str = r#"#!/bin/sh
[ "$1" = --strip-all ] || exit 2
cp "$2" "$3" && printf ' STRIPPED' >> "$3"
"#;

const PS: &str = r#"#!/bin/sh
echo "  PID USER       VSZ STAT COMMAND"
if [ -f "$FAKE_WAVE/run/watchdog" ]; then
    echo " $(cat "$FAKE_WAVE/run/watchdog") root      1520 S    /bin/sh /etc/init.d/S60waved start"
fi
if [ -f "$FAKE_WAVE/run/waved" ]; then
    echo " $(cat "$FAKE_WAVE/run/waved") root      300m S    /data/waved/waved"
fi
"#;

/// `killall waved` signals the fake waved, which then takes two `pidof`
/// polls to exit (the lift retracts first on the Wave).
const KILLALL: &str = r#"#!/bin/sh
sig=
for a; do case "$a" in -*) sig="$a ";; esac; done
status=1
for name; do
    case "$name" in -*) continue ;; esac
    echo "killall $sig$name" >> "$FAKE_WAVE/log"
    if [ "$name" = waved ] && [ -f "$FAKE_WAVE/run/waved" ]; then
        kill "$(cat "$FAKE_WAVE/run/waved")" 2>/dev/null
        echo 2 > "$FAKE_WAVE/run/waved.exiting"
        status=0
    fi
done
exit $status
"#;

const PIDOF: &str = r#"#!/bin/sh
run="$FAKE_WAVE/run"
[ -f "$run/$1" ] || exit 1
if [ -f "$run/$1.exiting" ]; then
    left=$(cat "$run/$1.exiting")
    if [ "$left" -le 0 ]; then
        rm -f "$run/$1" "$run/$1.exiting"
        echo "pidof $1: exited" >> "$FAKE_WAVE/log"
        exit 1
    fi
    echo $((left - 1)) > "$run/$1.exiting"
fi
echo "pidof $1: running" >> "$FAKE_WAVE/log"
cat "$run/$1"
"#;

const START_STOP_DAEMON: &str = r#"#!/bin/sh
printf 'start-stop-daemon %s\n' "$*" | sed "s#$FAKE_WAVE##g" >> "$FAKE_WAVE/log"
"#;

const SLEEP: &str = "#!/bin/sh\nexit 0\n";

/// `cp` that, with `$FAKE_DATA_FULL`, runs out of space half-way through
/// copying the new binary (`waved.new`) to `waved`. The old one still fits.
const CP: &str = r#"#!/bin/sh
for last; do :; done
if [ -n "$FAKE_DATA_FULL" ] && [ "${1##*/}" = waved.new ] && [ "${last##*/}" = waved ]; then
    printf 'half a binary' > "$last"
    echo "cp: write error: No space left on device" >&2
    exit 1
fi
exec /bin/cp "$@"
"#;

const MD5SUM: &str = r#"#!/bin/sh
for m in /sbin/md5sum /usr/bin/md5sum /bin/md5sum; do
    [ -x "$m" ] && exec "$m" "$@"
done
exec md5 -r "$@"
"#;

const NEW_BINARY: &str = "new waved binary";
const START_WATCHDOG: &str =
    "start-stop-daemon -S -b -m -p /tmp/S60waved.pid -x /bin/sh -- /etc/init.d/S60waved start";

fn deploy_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy")
}

fn write_exe(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

struct FakeWave {
    dir: tempfile::TempDir,
    processes: Vec<Child>,
    corrupt: Option<&'static str>,
    data_full: bool,
}

impl FakeWave {
    /// A checkout with a built binary, the fake tools, and an empty Wave.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repo = root.join("repo/waved");
        for name in ["install.sh", "S60waved", "iptables.rules"] {
            let to = repo.join("deploy").join(name);
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(deploy_dir().join(name), &to).unwrap();
        }
        fs::create_dir_all(repo.join("config")).unwrap();
        fs::copy(
            deploy_dir().join("../config/default.toml"),
            repo.join("config/default.toml"),
        )
        .unwrap();
        let binary = repo.join("target/aarch64-unknown-linux-gnu/release/waved");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, NEW_BINARY).unwrap();

        write_exe(&root.join("tools/adb"), ADB);
        write_exe(&root.join("tools/objcopy"), OBJCOPY);
        for (name, text) in [
            ("ps", PS),
            ("killall", KILLALL),
            ("pidof", PIDOF),
            ("start-stop-daemon", START_STOP_DAEMON),
            ("sleep", SLEEP),
            ("cp", CP),
            ("md5sum", MD5SUM),
        ] {
            write_exe(&root.join("wave-bin").join(name), text);
        }
        for dir in ["data", "tmp", "etc/init.d", "mnt", "run"] {
            fs::create_dir_all(root.join("wave").join(dir)).unwrap();
        }
        fs::write(root.join("wave/log"), "").unwrap();
        Self {
            dir,
            processes: Vec::new(),
            corrupt: None,
            data_full: false,
        }
    }

    /// A Wave that runs waved under the watchdog, with its own settings.
    fn running_waved() -> Self {
        let mut wave = Self::new();
        wave.put("data/waved/waved", "old waved", 0o755);
        wave.put(
            "data/waved/config.toml",
            "[device]\nname = \"Board room\"\n",
            0o600,
        );
        wave.put("etc/init.d/S60waved", "#!/bin/sh\necho old init\n", 0o755);
        wave.spawn("watchdog");
        wave.spawn("waved");
        wave
    }

    fn wave(&self, path: &str) -> PathBuf {
        self.dir.path().join("wave").join(path)
    }

    fn put(&self, path: &str, text: &str, mode: u32) {
        let path = self.wave(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn read(&self, path: &str) -> Option<String> {
        fs::read_to_string(self.wave(path)).ok()
    }

    /// A real process standing in for `name`, its pid where the fakes look.
    fn spawn(&mut self, name: &str) {
        let child = Command::new("sleep").arg("600").spawn().unwrap();
        fs::write(self.wave("run").join(name), child.id().to_string()).unwrap();
        self.processes.push(child);
    }

    /// Whether the process standing in for `name` (the n-th spawned) ended
    /// within a second.
    fn ended(&mut self, index: usize) -> bool {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if self.processes[index].try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn install(&self) -> Output {
        let root = self.dir.path();
        let path = format!(
            "{}:{}",
            root.join("tools").display(),
            std::env::var("PATH").unwrap()
        );
        let mut command = Command::new("bash");
        command
            .arg(root.join("repo/waved/deploy/install.sh"))
            .current_dir(root)
            .env("PATH", path)
            .env("FAKE_WAVE", root.join("wave"))
            .env("FAKE_WAVE_BIN", root.join("wave-bin"))
            .env("OBJCOPY", root.join("tools/objcopy"))
            .env_remove("WAVED_BINARY");
        if let Some(corrupt) = self.corrupt {
            command.env("FAKE_ADB_CORRUPT", corrupt);
        }
        if self.data_full {
            command.env("FAKE_DATA_FULL", "1");
        }
        command.output().unwrap()
    }

    fn log(&self) -> Vec<String> {
        fs::read_to_string(self.wave("log"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Where the first log line containing `needle` is.
    fn at(&self, needle: &str) -> Option<usize> {
        self.log().iter().position(|line| line.contains(needle))
    }

    /// Where the fake `start-stop-daemon` ran (not the `adb shell` that asked).
    fn watchdog_started(&self) -> Option<usize> {
        self.log()
            .iter()
            .position(|line| line.starts_with("start-stop-daemon "))
    }

    fn backups(&self) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(self.wave("data/waved"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                name.starts_with("config.toml.before-")
            })
            .collect();
        found.sort();
        found
    }
}

impl Drop for FakeWave {
    fn drop(&mut self) {
        for child in &mut self.processes {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn show(out: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn an_update_keeps_the_settings_and_swaps_a_stripped_binary_once_waved_has_exited() {
    let mut wave = FakeWave::running_waved();
    let out = wave.install();
    assert!(out.status.success(), "{}", show(&out));
    let log = wave.log().join("\n");

    // The settings on the Wave stay, and a copy is kept first.
    assert_eq!(
        wave.read("data/waved/config.toml").as_deref(),
        Some("[device]\nname = \"Board room\"\n"),
        "{log}"
    );
    let backups = wave.backups();
    assert_eq!(backups.len(), 1, "{log}");
    assert_eq!(
        fs::read_to_string(&backups[0]).unwrap(),
        "[device]\nname = \"Board room\"\n"
    );
    assert_eq!(mode(&backups[0]), 0o600, "it holds the API token");

    // The stripped binary, never pushed over the running one.
    assert_eq!(
        wave.read("data/waved/waved").as_deref(),
        Some("new waved binary STRIPPED")
    );
    assert_eq!(mode(&wave.wave("data/waved/waved")), 0o755);
    for line in wave.log().iter().filter(|l| l.starts_with("adb push ")) {
        let to = line.rsplit(' ').next().unwrap();
        assert!(to.starts_with("/tmp/"), "pushed straight to {to}:\n{log}");
    }

    // The init script, the same way.
    let init = fs::read_to_string(deploy_dir().join("S60waved")).unwrap();
    assert_eq!(wave.read("etc/init.d/S60waved"), Some(init));
    assert_eq!(mode(&wave.wave("etc/init.d/S60waved")), 0o755);
    assert!(!wave.wave("etc/init.d/.S60waved.new").exists());
    assert_eq!(
        wave.read("data/waved/iptables.rules"),
        fs::read_to_string(deploy_dir().join("iptables.rules")).ok()
    );

    // Watchdog stopped, waved gone, then the swap, then the watchdog again.
    assert!(wave.ended(0), "the watchdog was stopped:\n{log}");
    let exited = wave.at("pidof waved: exited").expect(&log);
    let swap = wave.at("/tmp/waved.new waved").expect(&log);
    let start = wave.watchdog_started().expect(&log);
    assert!(exited < swap && swap < start, "{log}");
    assert_eq!(wave.log()[start], START_WATCHDOG);
}

#[test]
fn a_first_install_writes_the_default_settings_and_asks_for_a_reboot() {
    let wave = FakeWave::new();
    let out = wave.install();
    assert!(out.status.success(), "{}", show(&out));
    let log = wave.log().join("\n");
    let default = fs::read_to_string(deploy_dir().join("../config/default.toml")).unwrap();
    assert_eq!(wave.read("data/waved/config.toml"), Some(default));
    assert_eq!(mode(&wave.wave("data/waved/config.toml")), 0o600);
    assert!(wave.backups().is_empty());
    assert_eq!(
        wave.read("data/waved/waved").as_deref(),
        Some("new waved binary STRIPPED")
    );
    assert!(wave.read("etc/init.d/S60waved").is_some());
    assert_eq!(
        wave.read("data/overlay/upper/etc/init.d/S60pspapp_daemon")
            .as_deref(),
        Some("#!/bin/sh\n")
    );
    assert!(wave.watchdog_started().is_none(), "{log}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("adb reboot"));
}

#[test]
fn a_damaged_push_changes_nothing_and_leaves_waved_running() {
    let mut wave = FakeWave::running_waved();
    wave.corrupt = Some("/tmp/waved.new");
    let out = wave.install();
    assert!(!out.status.success(), "{}", show(&out));
    let log = wave.log().join("\n");
    assert_eq!(wave.read("data/waved/waved").as_deref(), Some("old waved"));
    assert!(wave.at("killall").is_none(), "{log}");
    assert!(!wave.ended(0), "the watchdog still runs");
    assert!(wave.backups().is_empty(), "{log}");
}

#[test]
fn a_binary_that_does_not_fit_puts_the_old_one_back_and_restarts_the_watchdog() {
    let mut wave = FakeWave::running_waved();
    wave.data_full = true;
    let out = wave.install();
    assert!(!out.status.success(), "{}", show(&out));
    let log = wave.log().join("\n");
    assert_eq!(
        wave.read("data/waved/waved").as_deref(),
        Some("old waved"),
        "{}\n{log}",
        show(&out)
    );
    assert_eq!(mode(&wave.wave("data/waved/waved")), 0o755);
    let start = wave.watchdog_started().expect(&log);
    assert_eq!(wave.log()[start], START_WATCHDOG);
    assert_eq!(
        wave.read("data/waved/config.toml").as_deref(),
        Some("[device]\nname = \"Board room\"\n")
    );
}

#[test]
fn an_update_on_trial_is_left_alone() {
    let mut wave = FakeWave::running_waved();
    wave.put(
        "data/waved/update/trial",
        "version 0.10.0\nprevious 0.1.0\n",
        0o644,
    );
    let out = wave.install();
    assert!(!out.status.success(), "{}", show(&out));
    let log = wave.log().join("\n");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("on trial"),
        "{}",
        show(&out)
    );
    assert!(wave.at("adb push").is_none(), "{log}");
    assert_eq!(wave.read("data/waved/waved").as_deref(), Some("old waved"));
    assert!(!wave.ended(0), "the watchdog still runs");
}

#[test]
fn the_install_script_passes_shellcheck_when_it_is_installed() {
    let script = deploy_dir().join("install.sh");
    match Command::new("shellcheck").arg(&script).output() {
        Ok(out) => assert!(out.status.success(), "{}", show(&out)),
        Err(_) => eprintln!("shellcheck not installed; skipped"),
    }
}
