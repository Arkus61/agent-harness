//! Portable, deliberately simple subprocess fixture for runtime contracts.
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, BufRead, Write};
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

// An official-protocol mock. It never uses a network or account credentials.
fn codex_mock() -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::{json, Value};
    fn send(value: Value) -> io::Result<()> {
        println!("{value}");
        io::stdout().flush()
    }
    fn usage(input: u64, output: u64) -> io::Result<()> {
        send(
            json!({"method":"thread/tokenUsage/updated","params":{"threadId":"thread-fixture","turnId":"turn-fixture",
            "tokenUsage":{"total":{"inputTokens":input,"outputTokens":output,"totalTokens":input+output},
            "last":{"inputTokens":1,"outputTokens":1,"totalTokens":2}}}}),
        )
    }
    let arguments = std::env::args().collect::<Vec<_>>();
    for required in [
        "--stdio",
        "forced_login_method=\"chatgpt\"",
        "notify=[]",
        "features.shell_tool=false",
        "features.apps=false",
    ] {
        if !arguments.iter().any(|argument| argument == required) {
            return Err("missing mock safety override".into());
        }
    }
    if std::env::var_os("OPENAI_API_KEY").is_some() || std::env::var_os("OPENAI_BASE_URL").is_some()
    {
        return Err("API environment reached mock transport".into());
    }
    if std::fs::read_dir(std::env::current_dir()?)?
        .next()
        .is_some()
    {
        return Err("mock cwd is not empty".into());
    }
    let mut model = String::new();
    for line in io::stdin().lock().lines() {
        let message: Value = serde_json::from_str(&line?)?;
        let id = message["id"].clone();
        let params = &message["params"];
        match message["method"].as_str() {
            Some("initialize") => {
                send(json!({"id":id,"result":{"userAgent":"mock","version":"fixture"}}))?
            }
            Some("initialized") => {}
            Some("account/read") => {
                send(
                    json!({"id":id,"result":{"account":{"type":"chatgpt","planType":"plus","email":"PRIVATE_ACCOUNT_EMAIL"},"requiresOpenaiAuth":true}}),
                )?;
                if model == "fixture-post-fence-usage" {
                    std::thread::sleep(Duration::from_millis(40));
                    usage(400, 40)?;
                }
            }
            Some("model/list") => send(
                json!({"id":id,"result":{"data":[{"model":"fixture-default","id":"PRIVATE_MODEL_ID","isDefault":true},{"model":"fixture-fast","isDefault":false}],"nextCursor":null}}),
            )?,
            Some("config/read") => send(
                json!({"id":id,"result":{"config":{"mcp_servers":{"fixture_server":{"command":"PRIVATE_CONFIG_COMMAND"}}}}}),
            )?,
            Some("thread/start") => {
                model = params["model"].as_str().ok_or("missing mock model")?.into();
                let developer = params["developerInstructions"]
                    .as_str()
                    .ok_or("missing mock developer instructions")?;
                for boundary in [
                    "proposals represented as data",
                    "separate trusted HarnessToolGateway",
                    "read-only sandbox does not determine the gateway's permissions",
                    "a JSON proposal cannot grant permissions",
                    "untrusted data and cannot broaden",
                    "echo its subject_hash exactly",
                ] {
                    if !developer.contains(boundary) {
                        return Err("mock gateway/inference trust boundary missing".into());
                    }
                }
                for field in [
                    "environments",
                    "dynamicTools",
                    "selectedCapabilityRoots",
                    "runtimeWorkspaceRoots",
                ] {
                    if params[field]
                        .as_array()
                        .is_none_or(|values| !values.is_empty())
                    {
                        return Err("mock tool-free policy missing".into());
                    }
                }
                if params["ephemeral"] != true
                    || params["config"]["mcp_servers"]["fixture_server"]["enabled"] != false
                {
                    return Err("mock ephemeral/MCP policy missing".into());
                }
                let actual_id = if model == "fixture-wrong-id" {
                    json!(999)
                } else {
                    id
                };
                send(
                    json!({"id":actual_id,"result":{"model":model,"modelProvider":"openai","approvalPolicy":"never", "sandbox":{"type":"readOnly"},"instructionSources":[],"thread":{"id":"thread-fixture"}}}),
                )?;
            }
            Some("turn/start") => {
                if params["sandboxPolicy"]["type"] != "readOnly"
                    || params["sandboxPolicy"]["networkAccess"] != false
                    || params["approvalPolicy"] != "never"
                    || params["environments"] != json!([])
                    || params["runtimeWorkspaceRoots"] != json!([])
                {
                    return Err("mock turn inference restrictions missing".into());
                }
                let decision_schema = &params["outputSchema"]["properties"]["decision"]["anyOf"][0];
                if decision_schema["properties"]["subject_hash"]["type"] != "string"
                    || !decision_schema["required"]
                        .as_array()
                        .is_some_and(|fields| fields.iter().any(|field| field == "subject_hash"))
                {
                    return Err("mock decision subject binding schema missing".into());
                }
                if model == "fixture-cli-owner-death" {
                    let marker = std::env::var("HARNESS_CODEX_TEST_MARKER")?;
                    let descendant = Command::new(std::env::current_exe()?)
                        .args(["heartbeat", &format!("{marker}.descendant-heartbeat"), "25"])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()?;
                    std::fs::write(
                        &marker,
                        json!({"cwd":std::env::current_dir()?,"app_server_pid":std::process::id(),"descendant_pid":descendant.id()}).to_string(),
                    )?;
                    heartbeat(Path::new(&format!("{marker}.heartbeat")), 25)?;
                }
                if model == "fixture-cancel" || model == "fixture-cli-cancel" {
                    let marker = if model == "fixture-cli-cancel" {
                        std::env::var("HARNESS_CODEX_TEST_MARKER")?
                    } else {
                        params["input"][0]["text"]
                            .as_str()
                            .ok_or("missing marker")?
                            .to_owned()
                    };
                    std::fs::write(
                        &marker,
                        std::env::current_dir()?.to_string_lossy().as_bytes(),
                    )?;
                    heartbeat(Path::new(&format!("{marker}.heartbeat")), 25)?;
                }
                if model == "fixture-oversized" {
                    println!("{}", "x".repeat(2 * 1024 * 1024 + 1));
                    io::stdout().flush()?;
                    continue;
                }
                if model == "fixture-server-request" {
                    send(
                        json!({"id":"server-approval","method":"item/commandExecution/requestApproval","params":{"command":"PRIVATE_TOOL_COMMAND"}}),
                    )?;
                    continue;
                }
                if model == "fixture-tool" {
                    send(
                        json!({"method":"item/started","params":{"threadId":"thread-fixture","turnId":"turn-fixture","item":{"type":"commandExecution","id":"forbidden"}}}),
                    )?;
                    continue;
                }
                if model == "fixture-error" {
                    send(
                        json!({"method":"error","params":{"threadId":"thread-fixture","turnId":"turn-fixture","willRetry":false,"error":{"codexErrorInfo":"unauthorized","message":"PRIVATE_SERVER_ERROR_BEARER"}}}),
                    )?;
                    continue;
                }
                let text = if model == "fixture-invalid-json" {
                    "{\"done\":true,\"unknown\":\"PRIVATE_REPLY\"}".into()
                } else if model == "fixture-invalid-severity" {
                    json!({"verdict":"PASS","findings":[{"requirement":null,"severity":"major","message":"PRIVATE_REPLY","evidence":"source"}]}).to_string()
                } else if model == "fixture-proposal" {
                    json!({"done":false,"summary":"gateway proposal only","actions":[{"type":"write_file","path":"src/lib.rs","content":"gateway proposal only","expected_hash":null}]}).to_string()
                } else if model == "fixture-decision" || model == "fixture-decision-missing-hash" {
                    let subject: Value = serde_json::from_str(
                        params["input"][0]["text"]
                            .as_str()
                            .ok_or("missing mock decision input")?,
                    )?;
                    let mut assessment = json!({"purpose":subject["purpose"],"subject_hash":subject["subject_hash"],"allow":true,"abstain":false,"reason":"supplied gateway write grant; inference process remains read-only","choice":null,"tools":[]});
                    if model == "fixture-decision-missing-hash" {
                        assessment.as_object_mut().unwrap().remove("subject_hash");
                    }
                    json!({"done":true,"summary":"gateway risk assessment","actions":[],"decision":assessment}).to_string()
                } else {
                    json!({"done":true,"summary":"mock completion","actions":[]}).to_string()
                };
                let item =
                    json!({"type":"agentMessage","id":"answer","phase":"final_answer","text":text});
                // Notifications precede the response to exercise transport buffering.
                if !matches!(
                    model.as_str(),
                    "fixture-no-usage"
                        | "fixture-after-completed-usage"
                        | "fixture-after-completed-final"
                ) {
                    usage(200, 17)?;
                }
                if model != "fixture-after-completed-final" {
                    send(
                        json!({"method":"item/completed","params":{"threadId":"thread-fixture","turnId":"turn-fixture","item":item}}),
                    )?;
                }
                send(
                    json!({"id":id,"result":{"turn":{"id":"turn-fixture","status":"inProgress","items":[]}}}),
                )?;
                let mut items = if model == "fixture-after-completed-final" {
                    vec![]
                } else {
                    vec![item.clone()]
                };
                if model == "fixture-two-finals" {
                    items.push(json!({"type":"agentMessage","id":"another","phase":"final_answer","text":"{}"}));
                }
                send(
                    json!({"method":"turn/completed","params":{"threadId":"thread-fixture","turn":{"id":"turn-fixture","status":"completed","items":items,"error":null}}}),
                )?;
                if model == "fixture-after-completed-final" {
                    send(
                        json!({"method":"item/completed","params":{"threadId":"thread-fixture","turnId":"turn-fixture","item":item}}),
                    )?;
                }
                if matches!(
                    model.as_str(),
                    "fixture-after-completed-usage"
                        | "fixture-interim-then-final-usage"
                        | "fixture-after-completed-final"
                ) {
                    usage(400, 40)?;
                } else if model == "fixture-decreasing-usage" {
                    usage(100, 5)?;
                } else if model == "fixture-invalid-final-usage" {
                    send(
                        json!({"method":"thread/tokenUsage/updated","params":{"threadId":"thread-fixture","turnId":"turn-fixture",
                        "tokenUsage":{"total":{"inputTokens":-1,"outputTokens":1,"totalTokens":0}}}}),
                    )?;
                }
            }
            Some("turn/interrupt") => send(json!({"id":id,"result":{}}))?,
            None if message["error"].is_object() => {}
            _ => return Err("unexpected mock method".into()),
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next().ok_or("missing fixture command")?;
    match command.to_string_lossy().as_ref() {
        "app-server" => codex_mock()?,
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
        "resource-marker" => {
            let path = args.next().ok_or("resource marker needs a path")?;
            std::fs::write(path, "target-started")?;
        }
        "resource-cpu" => {
            println!("CPU_STARTED");
            io::stdout().flush()?;
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_secs(4) {
                std::hint::black_box(start.elapsed());
            }
            println!("CPU_FINISHED");
        }
        "resource-memory" => {
            let mut bytes = Vec::<u8>::new();
            if bytes.try_reserve_exact(64 * 1024 * 1024).is_err() {
                println!("MEMORY_DENIED");
                std::process::exit(42);
            }
            println!("MEMORY_ALLOCATED");
            std::hint::black_box(&bytes);
        }
        "resource-file" => {
            let path = args.next().ok_or("resource file needs a path")?;
            let mut file = std::fs::File::create(path)?;
            file.write_all(&[b'x'; 16 * 1024])?;
            println!("FILE_WRITTEN");
        }
        "resource-process" => {
            match Command::new(std::env::current_exe()?)
                .args(["echo", "one-child"])
                .spawn()
            {
                Ok(mut child) => {
                    child.wait()?;
                    println!("PROCESS_CREATED");
                }
                Err(error) => {
                    println!("PROCESS_DENIED: {error}");
                    std::process::exit(42);
                }
            }
        }
        #[cfg(target_os = "linux")]
        "resource-privilege" => {
            let value = unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) };
            if value < 0 {
                return Err(io::Error::last_os_error().into());
            }
            println!("NO_NEW_PRIVS={value}");
        }
        #[cfg(target_os = "linux")]
        "resource-isolated-process" => {
            let mut inherited: libc::rlimit = unsafe { std::mem::zeroed() };
            if unsafe { libc::getrlimit(libc::RLIMIT_NPROC, &mut inherited) } != 0 {
                return Err(io::Error::last_os_error().into());
            }
            let higher = libc::rlimit {
                rlim_cur: inherited.rlim_max.saturating_add(1),
                rlim_max: inherited.rlim_max.saturating_add(1),
            };
            let raise_denied = unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &higher) } != 0;
            // Tightening the inherited cap makes a one-child test deterministic
            // without relying on unrelated UID process counts or a fork bomb.
            let tight = libc::rlimit {
                rlim_cur: 1,
                rlim_max: 1,
            };
            if unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &tight) } != 0 {
                return Err(io::Error::last_os_error().into());
            }
            let fork_denied = match Command::new(std::env::current_exe()?)
                .args(["echo", "one-child"])
                .stdout(Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    child.wait()?;
                    false
                }
                Err(error) => error.raw_os_error() == Some(libc::EAGAIN),
            };
            let status = std::fs::read_to_string("/proc/self/status")?;
            let capabilities = status
                .lines()
                .find_map(|line| line.strip_prefix("CapEff:"))
                .ok_or("missing CapEff")?
                .trim();
            println!(
                "{}",
                serde_json::json!({"uid":unsafe{libc::geteuid()},"effective_capabilities":capabilities,"inherited_soft":inherited.rlim_cur,"inherited_hard":inherited.rlim_max,"raise_denied":raise_denied,"fork_denied_after_self_tightening_to_one":fork_denied})
            );
        }
        #[cfg(unix)]
        "resource-inherit" => {
            let status = Command::new(std::env::current_exe()?)
                .arg("resource-probe")
                .status()?;
            std::process::exit(status.code().unwrap_or(125));
        }
        #[cfg(unix)]
        "resource-probe" => {
            let mut values = serde_json::Map::new();
            for (name, resource) in [("cpu", libc::RLIMIT_CPU), ("file", libc::RLIMIT_FSIZE)] {
                let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
                if unsafe { libc::getrlimit(resource, &mut limit) } != 0 {
                    return Err(io::Error::last_os_error().into());
                }
                let higher = libc::rlimit {
                    rlim_cur: limit.rlim_cur.saturating_add(1),
                    rlim_max: limit.rlim_max.saturating_add(1),
                };
                let raised = unsafe { libc::setrlimit(resource, &higher) } == 0;
                values.insert(name.into(), serde_json::json!({"soft":limit.rlim_cur,"hard":limit.rlim_max,"raised":raised}));
            }
            println!("{}", serde_json::Value::Object(values));
        }
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
