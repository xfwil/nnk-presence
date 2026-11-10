//! Minimal Discord RPC over named pipes. Talks to every running client
//! (Stable / PTB / Canary each own one `discord-ipc-N` pipe).

use std::fs::File;
use std::io::{Read, Write};

pub struct Activity {
    pub details: String,
    pub state: String,
    pub large_image: String,
    pub large_text: String,
    pub small_image: String,
    pub small_text: String,
    pub start: u64,
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn activity_json(a: Option<&Activity>) -> String {
    let act = match a {
        None => "null".into(),
        Some(a) => format!(
            r#"{{"details":{},"state":{},"timestamps":{{"start":{}}},"assets":{{"large_image":{},"large_text":{},"small_image":{},"small_text":{}}}}}"#,
            esc(&a.details), esc(&a.state), a.start, esc(&a.large_image), esc(&a.large_text), esc(&a.small_image), esc(&a.small_text)
        ),
    };
    format!(r#"{{"cmd":"SET_ACTIVITY","args":{{"pid":{},"activity":{act}}},"nonce":"{}"}}"#, std::process::id(), rand_nonce())
}

fn rand_nonce() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

fn send(pipe: &mut File, op: u32, json: &str) -> std::io::Result<String> {
    let mut msg = Vec::with_capacity(8 + json.len());
    msg.extend_from_slice(&op.to_le_bytes());
    msg.extend_from_slice(&(json.len() as u32).to_le_bytes());
    msg.extend_from_slice(json.as_bytes());
    pipe.write_all(&msg)?;
    let mut hdr = [0u8; 8];
    pipe.read_exact(&mut hdr)?;
    let mut body = vec![0u8; u32::from_le_bytes(hdr[4..8].try_into().unwrap()) as usize];
    pipe.read_exact(&mut body)?;
    if u32::from_le_bytes(hdr[0..4].try_into().unwrap()) == 2 {
        return Err(std::io::Error::other("discord closed the connection"));
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// One connection per pipe; reconnects lazily and only resends on change.
pub struct Rpc {
    app_id: String,
    pipes: [Option<File>; 10],
    sent: [bool; 10],
}

impl Rpc {
    pub fn new(app_id: &str) -> Self {
        Rpc { app_id: app_id.into(), pipes: Default::default(), sent: [false; 10] }
    }

    /// Call when the activity changed; forces a resend everywhere.
    pub fn changed(&mut self) {
        self.sent = [false; 10];
    }

    /// Connects to new clients and pushes the activity where not yet sent. Returns connected count.
    pub fn tick(&mut self, a: Option<&Activity>) -> usize {
        let json = activity_json(a);
        for i in 0..10 {
            if self.pipes[i].is_none() {
                let Ok(mut p) = std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")) else { continue };
                if send(&mut p, 0, &format!(r#"{{"v":1,"client_id":{}}}"#, esc(&self.app_id))).is_err() {
                    continue;
                }
                self.pipes[i] = Some(p);
                self.sent[i] = false;
            }
            if !self.sent[i] {
                match send(self.pipes[i].as_mut().unwrap(), 1, &json) {
                    Ok(_) => self.sent[i] = true,
                    Err(e) => {
                        crate::log(&format!("discord-ipc-{i}: {e}"));
                        self.pipes[i] = None;
                    }
                }
            }
        }
        self.pipes.iter().filter(|p| p.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn json_escapes() {
        assert_eq!(super::esc("a\"b\\c\n"), r#""a\"b\\c\u000a""#);
        assert!(super::activity_json(None).contains(r#""activity":null"#));
    }
}
