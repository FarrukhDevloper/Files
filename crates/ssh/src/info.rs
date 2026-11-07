//! What a server is and how it's doing: one read-only exec of /proc and
//! friends. Every field is optional (non-Linux servers have no /proc).

use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// `PRETTY_NAME` from /etc/os-release, else `uname -s`.
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub uptime_secs: Option<u64>,
    /// 1, 5 and 15 minute load averages.
    pub load: Option<[f64; 3]>,
    pub cpus: Option<u32>,
    /// Bytes.
    pub mem_total: Option<u64>,
    pub mem_available: Option<u64>,
}

const MARK: &str = "@@vpsfiles ";

/// POSIX sh; reads only.
pub(crate) const SCRIPT: &str = "\
echo '@@vpsfiles loadavg'; cat /proc/loadavg 2>/dev/null
echo '@@vpsfiles meminfo'; grep -E '^(MemTotal|MemAvailable):' /proc/meminfo 2>/dev/null
echo '@@vpsfiles cpus'; nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null
echo '@@vpsfiles uptime'; cat /proc/uptime 2>/dev/null
echo '@@vpsfiles os'; cat /etc/os-release 2>/dev/null
echo '@@vpsfiles system'; uname -s 2>/dev/null
echo '@@vpsfiles kernel'; uname -r 2>/dev/null
";

fn sections(out: &str) -> impl Iterator<Item = (&str, &str)> {
    out.split(MARK).filter_map(|block| {
        let (name, body) = block.split_once('\n').unwrap_or((block, ""));
        (!name.is_empty()).then(|| (name.trim(), body.trim()))
    })
}

fn os_release(body: &str) -> Option<String> {
    let value = |key: &str| {
        body.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(|v| {
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v);
            v.replace("\\\"", "\"").replace("\\\\", "\\")
        })
    };
    value("PRETTY_NAME")
        .filter(|v| !v.is_empty())
        .or_else(|| Some(format!("{} {}", value("NAME")?, value("VERSION").unwrap_or_default()).trim().to_string()))
}

pub(crate) fn parse(out: &str) -> ServerInfo {
    let mut info = ServerInfo::default();
    let mut system = None;
    for (name, body) in sections(out) {
        let first = body.lines().next().unwrap_or("").trim();
        match name {
            "loadavg" => {
                let v: Vec<f64> = first.split_whitespace().take(3).filter_map(|x| x.parse().ok()).collect();
                info.load = <[f64; 3]>::try_from(v).ok();
            }
            "meminfo" => {
                for line in body.lines() {
                    let mut parts = line.split_whitespace();
                    let (Some(key), Some(kb)) = (parts.next(), parts.next().and_then(|v| v.parse::<u64>().ok())) else {
                        continue;
                    };
                    match key {
                        "MemTotal:" => info.mem_total = Some(kb * 1024),
                        "MemAvailable:" => info.mem_available = Some(kb * 1024),
                        _ => {}
                    }
                }
            }
            "cpus" => info.cpus = first.parse().ok().filter(|&n| n > 0),
            "uptime" => {
                info.uptime_secs = first.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()).map(|v| v as u64)
            }
            "os" => info.os = os_release(body).filter(|v| !v.is_empty()),
            "system" => system = Some(first.to_string()).filter(|v| !v.is_empty()),
            "kernel" => info.kernel = Some(first.to_string()).filter(|v| !v.is_empty()),
            _ => {}
        }
    }
    info.os = info.os.or(system);
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_linux_server() {
        let out = "@@vpsfiles loadavg\n0.52 0.31 0.20 1/234 5678\n\
            @@vpsfiles meminfo\nMemTotal:        2000000 kB\nMemAvailable:    1500000 kB\n\
            @@vpsfiles cpus\n4\n\
            @@vpsfiles uptime\n123456.78 400000.00\n\
            @@vpsfiles os\nNAME=\"Ubuntu\"\nVERSION=\"24.04 LTS\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n\
            @@vpsfiles system\nLinux\n\
            @@vpsfiles kernel\n6.8.0-45-generic\n";
        let info = parse(out);
        assert_eq!(
            info,
            ServerInfo {
                os: Some("Ubuntu 24.04.1 LTS".into()),
                kernel: Some("6.8.0-45-generic".into()),
                uptime_secs: Some(123456),
                load: Some([0.52, 0.31, 0.20]),
                cpus: Some(4),
                mem_total: Some(2_048_000_000),
                mem_available: Some(1_536_000_000),
            }
        );
        let json = serde_json::to_value(&info).unwrap();
        for key in ["os", "kernel", "uptimeSecs", "load", "cpus", "memTotal", "memAvailable"] {
            assert!(json.get(key).is_some(), "{key}");
        }
    }

    #[test]
    fn missing_parts_stay_empty() {
        let info = parse("@@vpsfiles loadavg\n@@vpsfiles meminfo\n@@vpsfiles cpus\n@@vpsfiles uptime\n@@vpsfiles os\n@@vpsfiles system\nFreeBSD\n@@vpsfiles kernel\n14.1-RELEASE\n");
        assert_eq!(info.os.as_deref(), Some("FreeBSD"));
        assert_eq!(info.kernel.as_deref(), Some("14.1-RELEASE"));
        assert_eq!((info.load, info.cpus, info.mem_total, info.uptime_secs), (None, None, None, None));
        assert_eq!(parse("garbage"), ServerInfo::default());
        assert_eq!(os_release("NAME=Debian\nVERSION=\"12 (bookworm)\""), Some("Debian 12 (bookworm)".into()));
    }
}
