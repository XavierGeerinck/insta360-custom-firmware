//! waved-pack: make and sign waved update packages on the host
//! (`docs/UPDATES.md`). The private key never leaves the file you keep it in.
//!
//! ```text
//! waved-pack keygen <private-key-file>
//! waved-pack pubkey <private-key-file>
//! waved-pack pack --key <private-key-file> --waved <stripped binary>
//!                 [--include deploy/S60waved]... [--build <git id>]
//!                 [--out <dir>] [--allow-unstripped]
//! waved-pack inspect <package.wup> [--pubkey <hex>]
//! ```

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use waved_update::format::{self, read_header};
use waved_update::key;
use waved_update::release::{self, ReleaseInput};

const USAGE: &str = "usage:
  waved-pack keygen <private-key-file>
  waved-pack pubkey <private-key-file>
  waved-pack pack --key <private-key-file> --waved <binary> [--include <deploy file>]...
                  [--build <id>] [--out <dir>] [--allow-unstripped]
  waved-pack inspect <package.wup> [--pubkey <hex>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("keygen") => keygen(&args[1..]),
        Some("pubkey") => pubkey(&args[1..]),
        Some("pack") => pack(&args[1..]),
        Some("inspect") => inspect(&args[1..]),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("waved-pack: {message}");
            ExitCode::FAILURE
        }
    }
}

fn one_path(args: &[String]) -> Result<PathBuf, String> {
    match args {
        [path] => Ok(PathBuf::from(path)),
        _ => Err(USAGE.to_string()),
    }
}

/// A private key must not end up in a git work tree.
fn inside_git_tree(path: &Path) -> bool {
    let start = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let start = fs::canonicalize(if start.as_os_str().is_empty() {
        Path::new(".")
    } else {
        &start
    })
    .unwrap_or(start);
    start.ancestors().any(|dir| dir.join(".git").exists())
}

fn read_signing(path: &Path) -> Result<ed25519_dalek::SigningKey, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    key::parse_signing_key(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn keygen(args: &[String]) -> Result<(), String> {
    let path = one_path(args)?;
    if inside_git_tree(&path) {
        return Err(format!(
            "{} is inside a git work tree; keep the private key outside the repo",
            path.display()
        ));
    }
    let signing = key::generate().map_err(|e| e.to_string())?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(&path)
        .map_err(|e| format!("{}: {e} (refusing to overwrite a key)", path.display()))?;
    writeln!(file, "{}", key::signing_hex(&signing)).map_err(|e| e.to_string())?;
    let public = key::public_hex(&signing.verifying_key());
    println!(
        "private key: {} (keep it safe, outside the repo)",
        path.display()
    );
    println!("public key:  {public}");
    println!("build waved with: WAVED_UPDATE_PUBKEY={public}");
    Ok(())
}

fn pubkey(args: &[String]) -> Result<(), String> {
    let signing = read_signing(&one_path(args)?)?;
    println!("{}", key::public_hex(&signing.verifying_key()));
    Ok(())
}

fn pack(args: &[String]) -> Result<(), String> {
    let (mut key_path, mut waved, mut out, mut build) = (None, None, PathBuf::from("."), None);
    let mut includes = Vec::new();
    let mut allow_unstripped = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--key" => key_path = Some(PathBuf::from(value()?)),
            "--waved" => waved = Some(PathBuf::from(value()?)),
            "--include" => includes.push(PathBuf::from(value()?)),
            "--out" => out = PathBuf::from(value()?),
            "--build" => build = Some(value()?),
            "--allow-unstripped" => allow_unstripped = true,
            _ => return Err(format!("unknown option {arg}\n{USAGE}")),
        }
    }
    let key_path = key_path.ok_or("--key is required")?;
    let waved = waved.ok_or("--waved is required")?;
    let signing = read_signing(&key_path)?;
    let binary = fs::read(&waved).map_err(|e| format!("{}: {e}", waved.display()))?;
    let includes = includes
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| format!("{}: no file name", path.display()))?
                .to_string();
            let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok((name, bytes))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let build = build.unwrap_or_else(git_build_id);
    let created_unix_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let release = release::build(
        ReleaseInput {
            binary: &binary,
            includes,
            build,
            created_unix_s,
            allow_unstripped,
        },
        &signing,
    )
    .map_err(|e| e.to_string())?;

    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let package_path = out.join(&release.package_name);
    fs::write(&package_path, &release.package)
        .map_err(|e| format!("{}: {e}", package_path.display()))?;
    let index_path = out.join(waved_update::index::INDEX_NAME);
    fs::write(&index_path, release.index.to_json())
        .map_err(|e| format!("{}: {e}", index_path.display()))?;
    println!(
        "waved {} ({}): {} bytes, {} unpacked",
        release.index.version,
        if release.index.build.is_empty() {
            "no build id"
        } else {
            &release.index.build
        },
        release.index.size,
        release.index.installed_size
    );
    for file in &release.manifest.files {
        println!(
            "  {:<15} {:>10} -> {:>10} bytes",
            file.target.name(),
            file.size,
            file.stored_size
        );
    }
    println!(
        "wrote {} and {}",
        package_path.display(),
        index_path.display()
    );
    println!("copy both to the directory security.ota_server points at");
    Ok(())
}

fn git_build_id() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

fn inspect(args: &[String]) -> Result<(), String> {
    let (path, public) = match args {
        [path] => (PathBuf::from(path), None),
        [path, flag, hex] if flag == "--pubkey" => (
            PathBuf::from(path),
            Some(key::parse_public_key(hex).map_err(|e| e.to_string())?),
        ),
        _ => return Err(USAGE.to_string()),
    };
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("sha256 {}", format::sha256_hex(&bytes));
    match public {
        Some(public) => {
            let header = read_header(&mut bytes.as_slice(), &public).map_err(|e| e.to_string())?;
            println!("signature: good");
            println!(
                "{}",
                serde_json::to_string_pretty(&header.manifest).unwrap_or_default()
            );
        }
        None => {
            // Without a key, show the header unverified.
            let len = bytes
                .get(8..12)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap_or([0; 4])) as usize)
                .unwrap_or(0);
            let header = bytes
                .get(12..12 + len.min(format::MAX_HEADER as usize))
                .ok_or("not a waved update package")?;
            println!("signature: not checked (pass --pubkey <hex>)");
            println!("{}", String::from_utf8_lossy(header));
        }
    }
    Ok(())
}
