//! Say what a listening port is. Three sources, cheapest first:
//! a table of ports and owners macOS users will recognise, the owner's own
//! command line and working directory, and finally a short HTTP request to
//! the port itself, which fingerprints most dev servers from their headers
//! and markup. Nothing here guesses; an unknown port stays unlabelled.

use crate::ports::PortInfo;
use crate::procs::ProcTable;
use crate::system::home;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Ports that speak something other than HTTP; never probed.
const NEVER_PROBE: &[u16] = &[
    22, 25, 53, 88, 110, 143, 445, 548, 631, 993, 995, 3306, 5432, 5900, 6379, 11211, 27017,
];

fn known(p: &PortInfo) -> Option<String> {
    let owner = p.process.as_str();
    let by_owner = match owner {
        "ControlCenter" => Some("AirPlay Receiver (macOS)"),
        "rapportd" => Some("Handoff and Continuity (macOS)"),
        "sharingd" => Some("AirDrop and Sharing (macOS)"),
        "Spotify" => Some("Spotify Connect and local API"),
        "limactl" => Some(if p.port == 53 {
            "Lima VM DNS"
        } else {
            "Lima VM port forward"
        }),
        "Code Helper (Plugin)" => Some("VS Code extension host"),
        "Cursor Helper (Plugin)" => Some("Cursor extension host"),
        "com.docker.backend" => Some("Docker Desktop"),
        "ollama" => Some("Ollama"),
        "postgres" => Some("PostgreSQL"),
        "redis-server" => Some("Redis"),
        "mysqld" => Some("MySQL"),
        "mongod" => Some("MongoDB"),
        "nginx" => Some("nginx"),
        "caddy" => Some("Caddy"),
        _ => None,
    };
    if let Some(l) = by_owner {
        return Some(l.to_string());
    }
    let by_port = match p.port {
        22 => "SSH",
        53 => "DNS",
        631 => "CUPS printing",
        3306 => "MySQL",
        5432 => "PostgreSQL",
        5900 => "Screen Sharing (VNC)",
        6379 => "Redis",
        8384 => "Syncthing",
        9200 => "Elasticsearch",
        11434 => "Ollama",
        27017 => "MongoDB",
        _ => return None,
    };
    Some(by_port.to_string())
}

fn project_suffix(cwd: Option<&str>) -> String {
    let Some(c) = cwd else { return String::new() };
    if c == "/" {
        return String::new();
    }
    let shown = home()
        .map(|h| h.to_string_lossy().into_owned())
        .and_then(|h| c.strip_prefix(&h).map(|rest| format!("~{rest}")))
        .unwrap_or_else(|| c.to_string());
    format!(" · {shown}")
}

/// What the command line says. `argv` excludes nothing; `exe` is the
/// executable's file name.
pub fn from_command(exe: &str, argv: &[String], cwd: Option<&str>) -> Option<String> {
    let joined = argv.join(" ").to_ascii_lowercase();
    let has = |needle: &str| joined.contains(needle);
    let script = argv.iter().skip(1).find(|a| !a.starts_with('-')).map(|a| {
        std::path::Path::new(a)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| a.clone())
    });
    let lower = exe.to_ascii_lowercase();
    let base = if lower == "node" || lower == "bun" || lower == "deno" {
        if has("next dev") || has("next-server") || has("/next ") || has("next start") {
            "Next.js dev server".to_string()
        } else if has("vite") {
            "Vite dev server".to_string()
        } else if has("nuxt") {
            "Nuxt dev server".to_string()
        } else if has("astro") {
            "Astro dev server".to_string()
        } else if has("remix") {
            "Remix dev server".to_string()
        } else if has("svelte") {
            "SvelteKit dev server".to_string()
        } else if has("ng serve") || has("@angular") {
            "Angular dev server".to_string()
        } else if has("react-scripts") {
            "Create React App dev server".to_string()
        } else if has("webpack") {
            "webpack dev server".to_string()
        } else if has("wrangler") {
            "Cloudflare Wrangler dev".to_string()
        } else if has("vercel dev") {
            "Vercel dev".to_string()
        } else if has("netlify") {
            "Netlify dev".to_string()
        } else if has("storybook") {
            "Storybook".to_string()
        } else if has("modelcontextprotocol") || has("mcp-server") || has("mcp ") {
            "MCP server".to_string()
        } else if has("http-server") || has("/serve ") || has("live-server") {
            "Static file server".to_string()
        } else if has("nodemon") || has("ts-node") || has("tsx ") {
            format!(
                "{} app{}",
                exe,
                script.map(|s| format!(" ({s})")).unwrap_or_default()
            )
        } else {
            format!(
                "{}{}",
                exe,
                script.map(|s| format!(" {s}")).unwrap_or_default()
            )
        }
    } else if lower.starts_with("python") {
        if has("-m http.server") || has("simplehttpserver") {
            "Python static server".to_string()
        } else if has("uvicorn") || has("hypercorn") {
            "ASGI app (uvicorn)".to_string()
        } else if has("gunicorn") {
            "WSGI app (gunicorn)".to_string()
        } else if has("manage.py runserver") {
            "Django dev server".to_string()
        } else if has("flask") {
            "Flask dev server".to_string()
        } else if has("jupyter") {
            "Jupyter".to_string()
        } else if has("streamlit") {
            "Streamlit".to_string()
        } else if has("mkdocs") {
            "MkDocs dev server".to_string()
        } else if has("aider") {
            "Aider".to_string()
        } else {
            format!(
                "python{}",
                script.map(|s| format!(" {s}")).unwrap_or_default()
            )
        }
    } else if lower == "ruby" || lower == "puma" || lower == "rails" {
        if has("rails") || lower == "puma" {
            "Rails server".to_string()
        } else if has("jekyll") {
            "Jekyll".to_string()
        } else {
            format!(
                "ruby{}",
                script.map(|s| format!(" {s}")).unwrap_or_default()
            )
        }
    } else if lower == "php" || lower == "php-fpm" {
        if has(" -s ") {
            "PHP built-in server".to_string()
        } else {
            "PHP".to_string()
        }
    } else if lower == "java" {
        let jar = argv.iter().skip_while(|a| *a != "-jar").nth(1).cloned();
        format!("Java{}", jar.map(|j| format!(": {j}")).unwrap_or_default())
    } else if lower == "hugo" {
        "Hugo dev server".to_string()
    } else if lower == "cargo" {
        "Rust app (cargo run)".to_string()
    } else {
        return None;
    };
    Some(format!("{base}{}", project_suffix(cwd)))
}

/// Ask the port what it is. Bounded by `timeout` for connect, write, and
/// read each, so a silent port costs at most three timeouts.
pub fn probe_http(addr: SocketAddr, timeout: Duration) -> Option<String> {
    let mut s = TcpStream::connect_timeout(&addr, timeout).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    s.write_all(
        b"GET / HTTP/1.0\r\nHost: localhost\r\nUser-Agent: autotrim/0.1\r\nAccept: */*\r\n\r\n",
    )
    .ok()?;
    let mut buf = vec![0u8; 8192];
    let mut total = 0;
    while total < buf.len() {
        match s.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(_) => break,
        }
    }
    if total == 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&buf[..total]).into_owned();
    Some(fingerprint(&text))
}

fn fingerprint(text: &str) -> String {
    if !text.starts_with("HTTP/") {
        return "not HTTP".to_string();
    }
    let lower = text.to_ascii_lowercase();
    let header = |name: &str| -> Option<String> {
        lower
            .lines()
            .take_while(|l| !l.is_empty())
            .find_map(|l| l.strip_prefix(name).map(|v| v.trim().to_string()))
    };
    let powered = header("x-powered-by:");
    let server = header("server:");
    let body_says = if lower.contains("/@vite/client") {
        Some("Vite dev server")
    } else if lower.contains("/_next/") || powered.as_deref().is_some_and(|p| p.contains("next.js"))
    {
        Some("Next.js")
    } else if lower.contains("__nuxt") {
        Some("Nuxt")
    } else if lower.contains("astro-island") || lower.contains("astro-dev-toolbar") {
        Some("Astro")
    } else if lower.contains("webpack") {
        Some("webpack dev server")
    } else if lower.contains("storybook") {
        Some("Storybook")
    } else if lower.contains("\"jsonrpc\"") {
        Some("JSON-RPC service")
    } else {
        None
    };
    if let Some(b) = body_says {
        return b.to_string();
    }
    if let Some(p) = powered {
        return match p.as_str() {
            s if s.contains("express") => "Express app".to_string(),
            s if s.contains("php") => "PHP app".to_string(),
            s => format!("HTTP app ({s})"),
        };
    }
    if let Some(s) = server {
        return match s.as_str() {
            v if v.contains("simplehttp") => "Python static server".to_string(),
            v if v.contains("werkzeug") => "Flask app".to_string(),
            v if v.contains("wsgiserver") => "Django dev server".to_string(),
            v if v.contains("uvicorn") => "ASGI app (uvicorn)".to_string(),
            v if v.contains("gunicorn") => "WSGI app (gunicorn)".to_string(),
            v if v.contains("nginx") => "nginx".to_string(),
            v if v.contains("caddy") => "Caddy".to_string(),
            v if v.contains("jetty") || v.contains("tomcat") => "Java web app".to_string(),
            v => format!("HTTP server ({})", v.split('/').next().unwrap_or(v)),
        };
    }
    if let Some(start) = lower.find("<title>") {
        let rest = &text[start + 7..];
        if let Some(end) = rest.to_ascii_lowercase().find("</title>") {
            let title = rest[..end].trim();
            if !title.is_empty() {
                return format!("HTTP: {}", title.chars().take(48).collect::<String>());
            }
        }
    }
    "HTTP server".to_string()
}

/// Fill in `label` and `label_source` on every port. `cache` remembers
/// probe results across daemon ticks so a port is asked once per lifetime.
pub fn label_all(
    ports: &mut [PortInfo],
    table: &ProcTable,
    probe: bool,
    timeout: Duration,
    cache: Option<&mut HashMap<String, String>>,
) {
    let mut cache = cache;
    // Cheap sources first.
    for p in ports.iter_mut() {
        if let Some(l) = known(p) {
            p.label = Some(l);
            p.label_source = Some("known".to_string());
            continue;
        }
        let proc_ = table.get(p.pid);
        let cwd = proc_
            .and_then(|x| x.cwd.as_ref())
            .map(|c| c.to_string_lossy().into_owned());
        if let Some(pr) = proc_
            && let Some(l) = from_command(&pr.exe_name(), &pr.cmd, cwd.as_deref())
        {
            p.label = Some(l);
            p.label_source = Some("process".to_string());
        }
    }
    if !probe {
        return;
    }
    // Probe what is still vague, in parallel, bounded by the timeout.
    let mut todo: Vec<usize> = Vec::new();
    for (i, p) in ports.iter().enumerate() {
        let vague = p.label.is_none()
            || p.label_source.as_deref() == Some("process")
                && p.label.as_deref().is_some_and(|l| {
                    l.starts_with("node")
                        || l.starts_with("python")
                        || l.starts_with("ruby")
                        || l.starts_with("bun")
                        || l.starts_with("deno")
                });
        if !vague || p.protocol != "tcp" || NEVER_PROBE.contains(&p.port) {
            continue;
        }
        if p.owner_managed && p.label.is_some() {
            continue;
        }
        let key = format!("{}:{}", p.pid, p.port);
        if let Some(c) = cache.as_deref()
            && let Some(l) = c.get(&key)
        {
            let _ = l;
            continue;
        }
        todo.push(i);
    }
    // Apply cached results.
    if let Some(c) = cache.as_deref() {
        for p in ports.iter_mut() {
            let key = format!("{}:{}", p.pid, p.port);
            if let Some(l) = c.get(&key)
                && (p.label.is_none() || p.label_source.as_deref() == Some("process"))
            {
                p.label = Some(l.clone());
                p.label_source = Some("probe".to_string());
            }
        }
    }
    if todo.is_empty() {
        return;
    }
    let results: Vec<(usize, Option<String>)> = std::thread::scope(|sc| {
        let handles: Vec<_> = todo
            .iter()
            .map(|&i| {
                let port = ports[i].port;
                let host = if ports[i].addr == "*" {
                    "127.0.0.1".to_string()
                } else {
                    ports[i].addr.clone()
                };
                sc.spawn(move || {
                    let addr: Option<SocketAddr> = format!("{host}:{port}")
                        .parse()
                        .ok()
                        .or_else(|| format!("[{host}]:{port}").parse().ok());
                    (i, addr.and_then(|a| probe_http(a, timeout)))
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    for (i, label) in results {
        let p = &mut ports[i];
        let key = format!("{}:{}", p.pid, p.port);
        if let Some(l) = label {
            let full = if l == "not HTTP" || l == "HTTP server" {
                // Keep the process-derived label, note what the probe saw.
                match &p.label {
                    Some(existing) => format!("{existing} ({l})"),
                    None => l.clone(),
                }
            } else {
                format!(
                    "{l}{}",
                    project_suffix(
                        table
                            .get(p.pid)
                            .and_then(|x| x.cwd.as_ref())
                            .map(|c| c.to_string_lossy().into_owned())
                            .as_deref()
                    )
                )
            };
            if let Some(c) = cache.as_deref_mut() {
                c.insert(key, full.clone());
            }
            p.label = Some(full);
            p.label_source = Some("probe".to_string());
        } else if let Some(c) = cache.as_deref_mut() {
            // Silent port: remember not to ask again this lifetime.
            c.insert(
                key,
                p.label.clone().unwrap_or_else(|| "no response".to_string()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn labels_dev_servers_from_argv() {
        assert_eq!(
            from_command(
                "node",
                &args("node /x/node_modules/.bin/next dev"),
                Some("/")
            )
            .as_deref(),
            Some("Next.js dev server")
        );
        assert!(
            from_command("node", &args("node node_modules/vite/bin/vite.js"), None)
                .unwrap()
                .starts_with("Vite dev server")
        );
        assert_eq!(
            from_command("python3", &args("python3 -m http.server 8000"), None).as_deref(),
            Some("Python static server")
        );
        assert_eq!(
            from_command("node", &args("node src/watch.mjs --dry"), None).as_deref(),
            Some("node watch.mjs")
        );
        assert!(from_command("limactl", &args("limactl"), None).is_none());
    }

    #[test]
    fn fingerprints_responses() {
        assert_eq!(
            fingerprint("HTTP/1.1 200 OK\r\nX-Powered-By: Express\r\n\r\nhi"),
            "Express app"
        );
        assert_eq!(
            fingerprint("HTTP/1.1 200 OK\r\n\r\n<script src=\"/@vite/client\">"),
            "Vite dev server"
        );
        assert_eq!(
            fingerprint("HTTP/1.0 200 OK\r\nServer: SimpleHTTP/0.6 Python/3.12\r\n\r\n"),
            "Python static server"
        );
        assert_eq!(
            fingerprint("HTTP/1.1 200 OK\r\n\r\n<html><title>My App</title>"),
            "HTTP: My App"
        );
        assert_eq!(fingerprint("SSH-2.0-OpenSSH"), "not HTTP");
    }
}
