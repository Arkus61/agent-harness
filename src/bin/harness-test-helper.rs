//! Portable, deliberately simple subprocess fixture for runtime contracts.
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

fn number(value: Option<OsString>, default: u64) -> Result<u64, Box<dyn std::error::Error>> {
    match value {
        Some(value) => Ok(value.to_string_lossy().parse()?),
        None => Ok(default),
    }
}

fn heartbeat(path: &Path, interval_ms: u64) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    loop {
        writeln!(file, "{}", std::process::id())?;
        file.flush()?;
        std::thread::sleep(Duration::from_millis(interval_ms));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next().ok_or("missing fixture command")?;
    match command.to_string_lossy().as_ref() {
        "stdoutflood" => {
            let mut remaining = number(args.next(), 1_048_576)?;
            let stdout = io::stdout();
            let mut output = stdout.lock();
            let buffer = [b'x'; 8192];
            while remaining > 0 {
                let count = remaining.min(buffer.len() as u64) as usize;
                output.write_all(&buffer[..count])?;
                remaining -= count as u64;
            }
            output.flush()?;
        }
        "sleep" => std::thread::sleep(Duration::from_millis(number(args.next(), 60_000)?)),
        "heartbeat" => {
            let path = args.next().ok_or("heartbeat requires an output path")?;
            heartbeat(Path::new(&path), number(args.next(), 25)?)?;
        }
        "spawn" => {
            let path = args.next().ok_or("spawn requires an output path")?;
            let mut child = Command::new(std::env::current_exe()?)
                .arg("heartbeat")
                .arg(path)
                .arg("25")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            println!("CHILD_PID={}", child.id());
            io::stdout().flush()?;
            child.wait()?;
        }
        "echo" => {
            for argument in args {
                println!("{}", argument.to_string_lossy());
            }
        }
        other => return Err(format!("unknown fixture command: {other}").into()),
    }
    Ok(())
}
