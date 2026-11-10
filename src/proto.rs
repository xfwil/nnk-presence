//! NNK Cross Worlds game-server protocol (TCP :12000).
//! Frame = [len: u24 LE, incl. header][op: u8][payload XOR KEY]. Offsets: see FINDINGS.md.

pub const KEY: [u8; 8] = [0x84, 0x20, 0xd0, 0xc9, 0x78, 0xb3, 0x9a, 0xa7];
const MAX_FRAME: usize = 1 << 20;
const SYNC_MAX: usize = 0x10000; // frames used to (re)sync must be small to avoid false positives
const SYNC_FRAMES: usize = 4;

pub const CLASSES: [&str; 6] = ["Unknown", "Swordsman", "Engineer", "Witch", "Rogue", "Destroyer"];

pub fn class_name(id: u32) -> &'static str {
    CLASSES.get(id as usize).copied().unwrap_or("Unknown")
}

/// (name, World.Type, World.PkType) from the game's World table.
pub fn world(id: u32) -> Option<(&'static str, u32, u32)> {
    include_str!("worlds.tsv").lines().find_map(|l| {
        let mut f = l.split('\t');
        (f.next()?.parse::<u32>().ok()? == id).then_some(())?;
        Some((f.next()?, f.next()?.parse().ok()?, f.next()?.parse().ok()?))
    })
}

/// Type 0 is the open world: PkType 4 = safe town, otherwise a hunting field.
/// Everything else (dungeons, raids, arenas, events) is shown by its own name.
pub fn activity_label(id: u32) -> Option<String> {
    let (name, kind, pk) = world(id)?;
    Some(match (kind, pk) {
        (0, 4) => format!("In {name}"),
        (0, _) => format!("Farming at {name}"),
        _ => name.to_string(),
    })
}

/// Open-world maps have channels; instanced content doesn't show one.
pub fn is_open_world(id: u32) -> bool {
    world(id).is_some_and(|w| w.1 == 0)
}

/// Splits one TCP direction's byte stream into decrypted frames.
#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
    synced: bool,
}

enum Sync {
    Yes,
    No,
    NeedMore,
}

fn u24(b: &[u8]) -> usize {
    b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16
}

fn chain(b: &[u8]) -> Sync {
    let mut i = 0;
    for _ in 0..SYNC_FRAMES {
        if i + 4 > b.len() {
            return Sync::NeedMore;
        }
        let n = u24(&b[i..]);
        if !(4..=SYNC_MAX).contains(&n) {
            return Sync::No;
        }
        i += n;
        if i > b.len() {
            return Sync::NeedMore;
        }
    }
    Sync::Yes
}

impl Framer {
    /// `synced = true` when the stream starts at a frame boundary (seen the SYN).
    pub fn reset(&mut self, synced: bool) {
        self.buf.clear();
        self.synced = synced;
    }

    pub fn push(&mut self, data: &[u8], mut on_frame: impl FnMut(u8, &[u8])) {
        self.buf.extend_from_slice(data);
        let mut pos = 0;
        loop {
            if !self.synced {
                // first offset with a complete valid chain wins; an undecidable (NeedMore)
                // offset only matters if nothing later proves itself
                let mut wait = None;
                let mut found = None;
                for i in pos..self.buf.len().saturating_sub(3) {
                    match chain(&self.buf[i..]) {
                        Sync::Yes => {
                            found = Some(i);
                            break;
                        }
                        Sync::NeedMore => wait = wait.or(Some(i)),
                        Sync::No => {}
                    }
                }
                match found {
                    Some(i) => pos = i,
                    None => {
                        pos = wait.unwrap_or(self.buf.len().saturating_sub(3));
                        break;
                    }
                }
                self.synced = true;
            }
            let b = &self.buf[pos..];
            if b.len() < 4 {
                break;
            }
            let n = u24(b);
            if !(4..=MAX_FRAME).contains(&n) {
                self.synced = false;
                pos += 1;
                continue;
            }
            if b.len() < n {
                break;
            }
            let p: Vec<u8> = b[4..n].iter().enumerate().map(|(i, x)| x ^ KEY[i & 7]).collect();
            on_frame(b[3], &p);
            pos += n;
        }
        self.buf.drain(..pos);
        if !self.synced && self.buf.len() > MAX_FRAME {
            self.buf.clear(); // ponytail: garbage never syncs; drop and wait for fresh data
        }
    }
}

#[derive(Default, Clone, PartialEq, Debug)]
pub struct Char {
    pub id: u64,
    pub name: String,
    pub class: u32,
    pub level: u32,
    pub cp: u32,
    pub world: u32,
    pub channel: u16,
    pub deck: Option<u8>,
}

/// Header of the own-character summary sent on every enter-world (also on re-select).
const C1_HEAD: [u8; 9] = [0, 0, 0, 0x10, 0, 0, 0, 0x01, 0];

fn u16_at(p: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(p.get(i..i + 2)?.try_into().ok()?))
}
fn u32_at(p: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(p.get(i..i + 4)?.try_into().ok()?))
}
fn u64_at(p: &[u8], i: usize) -> Option<u64> {
    Some(u64::from_le_bytes(p.get(i..i + 8)?.try_into().ok()?))
}

impl Char {
    fn set_world(&mut self, id: u32) -> Option<()> {
        world(id)?; // op bytes can collide with other messages: only accept real world ids
        self.world = id;
        Some(())
    }

    pub fn on_s2c(&mut self, op: u8, p: &[u8]) -> Option<()> {
        match op {
            // own char summary (locks the char id): id, class, level, CP as the last u32
            0xc1 => {
                let class = u32_at(p, 19)?;
                if p.len() < 31 || !p.starts_with(&C1_HEAD) || !(1..=5).contains(&class) {
                    return None;
                }
                self.id = u64_at(p, 9)?;
                self.class = class;
                self.level = u32_at(p, 23)?;
                self.cp = u32_at(p, p.len() - 4)?;
            }
            // full char info: id, u16-len name, class
            0xc9 => {
                let n = u16_at(p, 23)? as usize;
                if self.id == 0 || u64_at(p, 3)? != self.id || n == 0 {
                    return None;
                }
                self.name = String::from_utf8_lossy(p.get(25..25 + n)?).into_owned();
            }
            0x6d => {
                self.set_world(u32_at(p, 3)?)?;
                self.channel = u16_at(p, 7)?;
            }
            0x6b if p.len() == 8 => self.set_world(u32_at(p, 3)?)?,
            0x69 => self.set_world(u32_at(p, 11)?)?,
            // stat update; CP (stat 0x13d) is the last entry before the 5f79 trailer
            0xe4 => {
                let n = p.len();
                if n >= 21 && u64_at(p, 1)? == self.id && p.ends_with(&[0x5f, 0x79]) && p[n - 12..n - 10] == [0x3d, 0x01] {
                    self.cp = u32_at(p, n - 6)?;
                }
            }
            _ => {}
        }
        Some(())
    }

    /// C2S payload = [4 B rolling tag][u8 kind][u16 seq][body]; op byte rolls, so match by shape.
    pub fn on_c2s(&mut self, p: &[u8]) {
        match (p.len(), p.get(4)) {
            (9, Some(0)) => {
                if let Some(ch @ 1..=99) = u16_at(p, 7) {
                    self.channel = ch;
                }
            }
            (8, Some(2)) if p[7] < 10 => self.deck = Some(p[7]),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(bytes: &[u8], synced: bool, mut f: impl FnMut(u8, &[u8])) {
        let mut fr = Framer::default();
        fr.reset(synced);
        for chunk in bytes.chunks(1460) {
            fr.push(chunk, &mut f);
        }
    }

    // Real capture 2026-11-10 (name anonymised): Lv218 Engineer, Sagartha Ch.1, CP 10,216,846
    #[test]
    fn login_stream() {
        let mut c = Char::default();
        feed(include_bytes!("../tests/s2c_login.bin"), true, |op, p| {
            c.on_s2c(op, p);
        });
        assert_eq!(c.name, "Player");
        assert_eq!(class_name(c.class), "Engineer");
        assert_eq!(c.level, 218);
        assert_eq!(c.cp, 10_216_846);
        assert_eq!(world(c.world).map(|w| w.0), Some("Sagartha"));
        assert_eq!(c.channel, 1);
    }

    // Mid-stream capture: Ch2 -> Ch3, Deck 1 -> Deck 6 (0-based 0 -> 5)
    #[test]
    fn channel_and_deck_midstream() {
        let mut c = Char::default();
        let mut decks = vec![];
        feed(include_bytes!("../tests/c2s_midstream.bin"), false, |_, p| {
            c.on_c2s(p);
            if let Some(d) = c.deck {
                if decks.last() != Some(&d) {
                    decks.push(d);
                }
            }
        });
        assert_eq!(c.channel, 3);
        assert_eq!(decks, vec![0, 5]);
    }
}

#[cfg(test)]
mod label_tests {
    use super::activity_label as a;
    #[test]
    fn labels() {
        assert_eq!(a(1017).as_deref(), Some("Farming at Sagartha"));
        assert_eq!(a(2001).as_deref(), Some("In Evermore"));
        assert_eq!(a(3009).as_deref(), Some("Lava Valley"));
        assert_eq!(a(300602).as_deref(), Some("Temple Team Arena"));
        assert_eq!(a(10202).as_deref(), Some("Dream Witch (Normal)"));
    }
}
