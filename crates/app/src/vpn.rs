use crate::platform::{self, Event};
use hostnr_core::{dns, hosts::Hosts, packet};
use jni::{objects::{JClass, JObject, JValue}, sys::{jint, jlong}, JNIEnv};
use std::{fs::File, io::{Read, Write}, net::{TcpStream, UdpSocket}, os::fd::{AsRawFd, FromRawFd}, sync::{atomic::{AtomicU64, Ordering}, OnceLock, RwLock}, time::Duration};

static RULES: OnceLock<RwLock<Option<Hosts>>> = OnceLock::new();
static SESSION: AtomicU64 = AtomicU64::new(0);
pub static HITS: AtomicU64 = AtomicU64::new(0);
pub static FORWARDED: AtomicU64 = AtomicU64::new(0);

pub fn set_hosts(hosts: Option<Hosts>) {
    if let Ok(mut rules) = RULES.get_or_init(|| RwLock::new(None)).write() { *rules = hosts; }
}
pub fn running() -> bool {
    SESSION.load(Ordering::SeqCst) & 1 != 0
}

pub fn has_hosts() -> bool {
    RULES.get().and_then(|r| r.read().ok()).is_some_and(|r| r.is_some())
}

fn reserve_session() -> u64 {
    /* Even values reserve a session; the low bit marks an active TUN loop. */
    let old = SESSION.fetch_update(Ordering::SeqCst, Ordering::SeqCst,
        |value| Some((value & !1).wrapping_add(2))).unwrap_or_else(|value| value);
    (old & !1).wrapping_add(2)
}

fn protect(env: &mut JNIEnv, service: &JObject, fd: i32) -> Result<(), String> {
    let ok = env.call_method(service, "protect", "(I)Z", &[JValue::Int(fd)])
        .and_then(|v| v.z()).map_err(|e| { let _ = env.exception_clear(); e.to_string() })?;
    if ok { Ok(()) } else { Err("Failed to protect the upstream DNS socket".into()) }
}

fn upstream(env: &mut JNIEnv, service: &JObject, query: &[u8]) -> Option<Vec<u8>> {
    /* Validate the peer, transaction ID, and DNS question. */
    for server in ["1.1.1.1:53", "9.9.9.9:53"] {
        let mut try_udp = || -> Option<Vec<u8>> {
            let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
            protect(env, service, socket.as_raw_fd()).ok()?;
            socket.set_read_timeout(Some(Duration::from_millis(1500))).ok()?;
            socket.set_write_timeout(Some(Duration::from_millis(1500))).ok()?;
            socket.connect(server).ok()?;
            socket.send(query).ok()?;
            let mut buf = [0u8; 4096];
            let n = socket.recv(&mut buf).ok()?;
            if !dns::same_question(query, &buf[..n]) { return None; }
            Some(buf[..n].to_vec())
        };
        if let Some(response) = (try_udp)() {
            if response[2] & 2 != 0 {
                /*
                 * Retry truncated upstream UDP over TCP. The synthetic /32 is
                 * not the default route, so this socket cannot loop into TUN.
                 */
                if let Ok(mut stream) = TcpStream::connect_timeout(&server.parse().ok()?, Duration::from_millis(1500)) {
                    if protect(env, service, stream.as_raw_fd()).is_ok() {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
                        let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));
                        let result = (|| -> std::io::Result<Vec<u8>> {
                            stream.write_all(&(query.len() as u16).to_be_bytes())?;
                            stream.write_all(query)?;
                            let mut size = [0;2]; stream.read_exact(&mut size)?;
                            let mut body = vec![0; u16::from_be_bytes(size) as usize];
                            stream.read_exact(&mut body)?; Ok(body)
                        })();
                        if let Ok(body) = result { if dns::same_question(query, &body) { return Some(fit_response(query, body)); } }
                    }
                }
            }
            return Some(fit_response(query, response));
        }
    }
    None
}

fn fit_response(query: &[u8], response: Vec<u8>) -> Vec<u8> {
    if response.len() <= 1472 { return response; }
    /* Signal truncation instead of emitting fragmented IP packets. */
    let mut out = dns::failure(query).unwrap_or_default();
    if out.len() >= 12 { out[2] |= 2; out[3] &= 0xf0; }
    out
}

#[no_mangle]
pub extern "system" fn Java_app_hostnr_HostVpnService_nativePrepare(_env: JNIEnv, _class: JClass) -> jlong {
    reserve_session() as jlong
}

#[no_mangle]
pub extern "system" fn Java_app_hostnr_HostVpnService_nativeStop(_env: JNIEnv, _class: JClass) {
    reserve_session();
}

#[no_mangle]
pub extern "system" fn Java_app_hostnr_HostVpnService_nativeRun(mut env: JNIEnv, _class: JClass, fd: jint, token: jlong, service: JObject) {
    if fd < 0 { return; }
    /* Transfer detached ParcelFileDescriptor ownership to Rust exactly once. */
    let mut tun = unsafe { File::from_raw_fd(fd) };
    let generation = token as u64 | 1;
    if SESSION.compare_exchange(token as u64, generation, Ordering::SeqCst, Ordering::SeqCst).is_err() { return; }
    HITS.store(0, Ordering::Relaxed); FORWARDED.store(0, Ordering::Relaxed);
    let mut buffer = [0u8; 32768];
    while SESSION.load(Ordering::SeqCst) == generation {
        let mut poll = libc::pollfd { fd: tun.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ready = unsafe { libc::poll(&mut poll, 1, 200) };
        if ready < 0 { if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted { continue; } break; }
        if ready == 0 { continue; }
        if poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 { break; }
        let n = match tun.read(&mut buffer) { Ok(0) => break, Ok(n) => n, Err(_) => break };
        let original = &buffer[..n];
        let Some(query) = packet::dns_payload(original) else { continue; };
        if dns::question(query).is_none() { continue; }
        let local = RULES.get().and_then(|r| r.read().ok()).and_then(|r| r.as_ref().and_then(|h| dns::answer(h, query)));
        let answer = if let Some(answer) = local {
            HITS.fetch_add(1, Ordering::Relaxed); Some(answer)
        } else {
            FORWARDED.fetch_add(1, Ordering::Relaxed);
            upstream(&mut env, &service, query).or_else(|| dns::failure(query))
        };
        if SESSION.load(Ordering::SeqCst) != generation { break; }
        if let Some(reply) = answer.and_then(|d| packet::response(original, &d)) {
            if tun.write_all(&reply).is_err() { break; }
        }
    }
    if SESSION.compare_exchange(generation, generation.wrapping_add(1), Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        platform::push(Event::Message("VPN stopped; reconnect to resume".into()));
    }
}
