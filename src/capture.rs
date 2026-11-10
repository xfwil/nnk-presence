//! Passive sniffing via Npcap's wpcap.dll (loaded at runtime, no SDK needed) + TCP reassembly.

use crate::proto::{Char, Framer};
use crate::Shared;
use libloading::os::windows::{Library, LOAD_WITH_ALTERED_SEARCH_PATH};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Arc, Mutex};

const PORT: u16 = 12000;
const FILTER: &str = "tcp port 12000";

#[repr(C)]
struct PcapIf {
    next: *mut PcapIf,
    name: *const c_char,
    desc: *const c_char,
    addrs: *mut c_void,
    flags: u32,
}
#[repr(C)]
struct Bpf {
    len: u32,
    insns: *mut c_void,
}
#[repr(C)]
struct Hdr {
    sec: i32,
    usec: i32,
    caplen: u32,
    len: u32,
}

type FindAll = unsafe extern "C" fn(*mut *mut PcapIf, *mut c_char) -> c_int;
type OpenLive = unsafe extern "C" fn(*const c_char, c_int, c_int, c_int, *mut c_char) -> *mut c_void;
type Compile = unsafe extern "C" fn(*mut c_void, *mut Bpf, *const c_char, c_int, u32) -> c_int;
type SetFilter = unsafe extern "C" fn(*mut c_void, *mut Bpf) -> c_int;
type NextEx = unsafe extern "C" fn(*mut c_void, *mut *mut Hdr, *mut *const u8) -> c_int;
type Datalink = unsafe extern "C" fn(*mut c_void) -> c_int;

struct Pcap {
    _lib: Library,
    open: OpenLive,
    compile: Compile,
    set_filter: SetFilter,
    next: NextEx,
    datalink: Datalink,
}

/// Opens every non-loopback adapter and sniffs each on its own thread.
pub fn start(shared: Arc<Mutex<Shared>>) -> Result<Vec<String>, String> {
    let pcap = unsafe { load() }.map_err(|e| format!("Npcap not available: {e}"))?;
    let pcap: &'static Pcap = Box::leak(Box::new(pcap));
    let mut opened = vec![];
    for (dev, desc) in unsafe { devices()? } {
        if desc.to_lowercase().contains("loopback") {
            continue;
        }
        let mut err = [0 as c_char; 256];
        let cdev = CString::new(dev).unwrap();
        let h = unsafe { (pcap.open)(cdev.as_ptr(), 65535, 0, 200, err.as_mut_ptr()) };
        if h.is_null() {
            continue;
        }
        let mut prog = Bpf { len: 0, insns: std::ptr::null_mut() };
        let f = CString::new(FILTER).unwrap();
        unsafe {
            if (pcap.compile)(h, &mut prog, f.as_ptr(), 1, 0xffff_ffff) != 0 || (pcap.set_filter)(h, &mut prog) != 0 {
                continue;
            }
        }
        let link = unsafe { (pcap.datalink)(h) };
        let h = h as usize; // raw pointer isn't Send; the handle is used by this one thread only
        let sh = shared.clone();
        std::thread::spawn(move || sniff(pcap, h as *mut c_void, link, sh));
        opened.push(desc);
    }
    if opened.is_empty() {
        return Err("no capturable network adapter (run as Administrator?)".into());
    }
    Ok(opened)
}

/// Runs a saved capture through the same pipeline (debugging aid: `nnk-presence --replay file.pcapng`).
pub fn replay(path: &str) -> Result<Char, String> {
    type OpenOffline = unsafe extern "C" fn(*const c_char, *mut c_char) -> *mut c_void;
    let pcap = unsafe { load() }.map_err(|e| e.to_string())?;
    let open: OpenOffline = unsafe { *pcap._lib.get::<OpenOffline>(b"pcap_open_offline\0").map_err(|e| e.to_string())? };
    let mut err = [0 as c_char; 256];
    let p = CString::new(path).unwrap();
    let h = unsafe { open(p.as_ptr(), err.as_mut_ptr()) };
    if h.is_null() {
        return Err(unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().into_owned());
    }
    let shared = Arc::new(Mutex::new(Shared::default()));
    sniff(&pcap, h, unsafe { (pcap.datalink)(h) }, shared.clone());
    let ch = shared.lock().unwrap().ch.clone();
    Ok(ch)
}

unsafe fn load() -> Result<Pcap, libloading::Error> {
    let lib = Library::load_with_flags(r"C:\Windows\System32\Npcap\wpcap.dll", LOAD_WITH_ALTERED_SEARCH_PATH)?;
    Ok(Pcap {
        open: *lib.get::<OpenLive>(b"pcap_open_live\0")?,
        compile: *lib.get::<Compile>(b"pcap_compile\0")?,
        set_filter: *lib.get::<SetFilter>(b"pcap_setfilter\0")?,
        next: *lib.get::<NextEx>(b"pcap_next_ex\0")?,
        datalink: *lib.get::<Datalink>(b"pcap_datalink\0")?,
        _lib: lib,
    })
}

unsafe fn devices() -> Result<Vec<(String, String)>, String> {
    let lib = Library::load_with_flags(r"C:\Windows\System32\Npcap\wpcap.dll", LOAD_WITH_ALTERED_SEARCH_PATH).map_err(|e| e.to_string())?;
    let find: FindAll = *lib.get::<FindAll>(b"pcap_findalldevs\0").map_err(|e| e.to_string())?;
    let mut list: *mut PcapIf = std::ptr::null_mut();
    let mut err = [0 as c_char; 256];
    if find(&mut list, err.as_mut_ptr()) != 0 {
        return Err(CStr::from_ptr(err.as_ptr()).to_string_lossy().into_owned());
    }
    let mut out = vec![];
    let mut cur = list;
    while !cur.is_null() {
        let d = &*cur;
        let s = |p: *const c_char| if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() };
        out.push((s(d.name), s(d.desc)));
        cur = d.next;
    }
    // ponytail: list is leaked (pcap_freealldevs skipped), called once at startup
    Ok(out)
}

#[derive(Default)]
struct Flow {
    next: Option<u32>,
    pending: BTreeMap<u32, Vec<u8>>,
    framer: Framer,
}

type Key = ([u8; 4], u16, [u8; 4], u16);

fn sniff(pcap: &Pcap, h: *mut c_void, link: c_int, shared: Arc<Mutex<Shared>>) {
    let mut flows: HashMap<Key, Flow> = HashMap::new();
    loop {
        let mut hdr: *mut Hdr = std::ptr::null_mut();
        let mut data: *const u8 = std::ptr::null();
        match unsafe { (pcap.next)(h, &mut hdr, &mut data) } {
            1 => {}
            0 => continue,
            _ => return,
        }
        let pkt = unsafe { std::slice::from_raw_parts(data, (*hdr).caplen as usize) };
        if let Some(seg) = parse(link, pkt) {
            on_segment(&mut flows, seg, &shared);
        }
    }
}

struct Seg<'a> {
    key: Key,
    seq: u32,
    syn: bool,
    data: &'a [u8],
}

fn parse(link: c_int, pkt: &[u8]) -> Option<Seg<'_>> {
    let mut ip = match link {
        1 => {
            let mut off = 14;
            let mut ety = u16::from_be_bytes([*pkt.get(12)?, *pkt.get(13)?]);
            if ety == 0x8100 {
                ety = u16::from_be_bytes([*pkt.get(16)?, *pkt.get(17)?]);
                off = 18;
            }
            if ety != 0x0800 {
                return None;
            }
            pkt.get(off..)?
        }
        0 | 108 => pkt.get(4..)?,
        12 | 14 | 101 => pkt,
        _ => return None,
    };
    if ip.first()? >> 4 != 4 || *ip.get(9)? != 6 {
        return None;
    }
    let total = u16::from_be_bytes([ip[2], ip[3]]) as usize;
    ip = ip.get(..total.min(ip.len()))?;
    let ihl = ((ip[0] & 0xf) as usize) * 4;
    let tcp = ip.get(ihl..)?;
    let doff = ((*tcp.get(12)? >> 4) as usize) * 4;
    Some(Seg {
        key: (ip[12..16].try_into().ok()?, u16::from_be_bytes([tcp[0], tcp[1]]), ip[16..20].try_into().ok()?, u16::from_be_bytes([tcp[2], tcp[3]])),
        seq: u32::from_be_bytes(tcp[4..8].try_into().ok()?),
        syn: tcp[13] & 0x02 != 0,
        data: tcp.get(doff..)?,
    })
}

fn on_segment(flows: &mut HashMap<Key, Flow>, seg: Seg, shared: &Arc<Mutex<Shared>>) {
    let from_server = seg.key.1 == PORT;
    let flow = flows.entry(seg.key).or_default();
    if seg.syn {
        flow.next = Some(seg.seq.wrapping_add(1));
        flow.pending.clear();
        flow.framer.reset(true);
        if !from_server {
            shared.lock().unwrap().ch.fresh = true; // new game/lobby login
        }
        return;
    }
    if seg.data.is_empty() {
        return;
    }
    let next = *flow.next.get_or_insert(seg.seq); // joined mid-stream: framer resyncs itself
    let d = seg.seq.wrapping_sub(next) as i32;
    if d > 0 {
        flow.pending.insert(seg.seq, seg.data.to_vec());
        if flow.pending.len() > 256 {
            // a segment was lost for good: skip the gap
            let first = *flow.pending.keys().next().unwrap();
            flow.next = Some(first);
            flow.framer.reset(false);
        } else {
            return;
        }
    } else {
        deliver(flow, seg.seq, seg.data, from_server, shared);
    }
    while let Some(k) = flow.pending.keys().copied().find(|k| k.wrapping_sub(flow.next.unwrap()) as i32 <= 0) {
        let data = flow.pending.remove(&k).unwrap();
        deliver(flow, k, &data, from_server, shared);
    }
}

fn deliver(flow: &mut Flow, seq: u32, data: &[u8], from_server: bool, shared: &Arc<Mutex<Shared>>) {
    let next = flow.next.unwrap();
    let skip = next.wrapping_sub(seq) as usize; // overlap with what we already have
    if skip >= data.len() {
        return;
    }
    let data = &data[skip..];
    flow.next = Some(next.wrapping_add(data.len() as u32));
    let mut sh = shared.lock().unwrap();
    let Shared { ch, frames, .. } = &mut *sh;
    flow.framer.push(data, |op, p| {
        *frames += 1;
        if from_server {
            ch.on_s2c(op, p);
        } else {
            Char::on_c2s(ch, p);
        }
    });
}
