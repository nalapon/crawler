//! Public Bonjour browsing over AWDL. All DNSServiceRefs belong to one worker.
use super::network;
use rune::Waker;
use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_void},
    net::{Ipv6Addr, SocketAddrV6},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub detail: String,
    pub endpoint: SocketAddrV6,
    pub ready: bool,
}
#[derive(Debug)]
pub enum Update {
    Device(Device),
    Identified(Device),
    Removed(String),
    Failure(String),
}
type Service = *mut c_void;
type BrowseCallback = unsafe extern "C" fn(
    Service,
    u32,
    u32,
    i32,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut c_void,
);
type ResolveCallback = unsafe extern "C" fn(
    Service,
    u32,
    u32,
    i32,
    *const c_char,
    *const c_char,
    u16,
    u16,
    *const u8,
    *mut c_void,
);
type AddressCallback = unsafe extern "C" fn(
    Service,
    u32,
    u32,
    i32,
    *const c_char,
    *const libc::sockaddr,
    u32,
    *mut c_void,
);
unsafe extern "C" {
    fn DNSServiceBrowse(
        s: *mut Service,
        flags: u32,
        interface: u32,
        kind: *const c_char,
        domain: *const c_char,
        cb: BrowseCallback,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceResolve(
        s: *mut Service,
        flags: u32,
        interface: u32,
        name: *const c_char,
        kind: *const c_char,
        domain: *const c_char,
        cb: ResolveCallback,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceGetAddrInfo(
        s: *mut Service,
        flags: u32,
        interface: u32,
        protocol: u32,
        host: *const c_char,
        cb: AddressCallback,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceRefSockFD(s: Service) -> i32;
    fn DNSServiceProcessResult(s: Service) -> i32;
    fn DNSServiceRefDeallocate(s: Service);
}
const PEER_FLAGS: u32 = 0x100000 | 0x20000;
enum Event {
    Found {
        id: String,
        name: CString,
        kind: CString,
        domain: CString,
        interface: u32,
    },
    Removed(String),
    Resolved {
        id: String,
        host: CString,
        port: u16,
        interface: u32,
    },
    Address {
        id: String,
        address: SocketAddrV6,
    },
    Failure(i32),
}
struct Context {
    id: String,
    events: *mut Vec<Event>,
    port: u16,
}
struct OwnedService {
    raw: Service,
    _context: Box<Context>,
}
impl Drop for OwnedService {
    fn drop(&mut self) {
        unsafe {
            DNSServiceRefDeallocate(self.raw);
        }
    }
}
unsafe fn push(context: *mut c_void, event: Event) {
    unsafe {
        (&mut *(*(context as *mut Context)).events).push(event);
    }
}
unsafe extern "C" fn found(
    _: Service,
    flags: u32,
    interface: u32,
    error: i32,
    name: *const c_char,
    kind: *const c_char,
    domain: *const c_char,
    ctx: *mut c_void,
) {
    if error != 0 {
        unsafe {
            push(ctx, Event::Failure(error));
        }
        return;
    }
    if name.is_null() || kind.is_null() || domain.is_null() {
        return;
    }
    let (name, kind, domain) = unsafe {
        (
            CStr::from_ptr(name).to_owned(),
            CStr::from_ptr(kind).to_owned(),
            CStr::from_ptr(domain).to_owned(),
        )
    };
    let id = format!(
        "{}.{}{}%{interface}",
        name.to_string_lossy(),
        kind.to_string_lossy(),
        domain.to_string_lossy()
    );
    unsafe {
        push(
            ctx,
            if flags & 2 == 0 {
                Event::Removed(id)
            } else {
                Event::Found {
                    id,
                    name,
                    kind,
                    domain,
                    interface,
                }
            },
        );
    }
}
unsafe extern "C" fn resolved(
    _: Service,
    _: u32,
    interface: u32,
    error: i32,
    _: *const c_char,
    host: *const c_char,
    port: u16,
    _: u16,
    _: *const u8,
    ctx: *mut c_void,
) {
    if error != 0 || host.is_null() {
        return;
    }
    unsafe {
        push(
            ctx,
            Event::Resolved {
                id: (*(ctx as *mut Context)).id.clone(),
                host: CStr::from_ptr(host).to_owned(),
                port: u16::from_be(port),
                interface,
            },
        );
    }
}
unsafe extern "C" fn address(
    _: Service,
    flags: u32,
    interface: u32,
    error: i32,
    _: *const c_char,
    addr: *const libc::sockaddr,
    _: u32,
    ctx: *mut c_void,
) {
    if error != 0
        || flags & 2 == 0
        || addr.is_null()
        || unsafe { (*addr).sa_family } as i32 != libc::AF_INET6
    {
        return;
    }
    let v6 = unsafe { &*(addr as *const libc::sockaddr_in6) };
    let ip = Ipv6Addr::from(v6.sin6_addr.s6_addr);
    let context = unsafe { &*(ctx as *const Context) };
    unsafe {
        push(
            ctx,
            Event::Address {
                id: context.id.clone(),
                address: SocketAddrV6::new(
                    ip,
                    context.port,
                    0,
                    if ip.is_unicast_link_local() {
                        interface
                    } else {
                        0
                    },
                ),
            },
        );
    }
}

pub struct Discovery {
    cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Update>,
}
impl Drop for Discovery {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Discovery {
    pub fn start(waker: Waker) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || run(stop, tx, waker));
        Self { cancel, rx }
    }
}
fn run(cancel: Arc<AtomicBool>, tx: mpsc::Sender<Update>, waker: Waker) {
    let publish = |u| {
        let _ = tx.send(u);
        waker();
    };
    // Context pointers are boxed and remain stable until their service is deallocated.
    let mut events = Vec::new();
    let mut ctx = Box::new(Context {
        id: String::new(),
        events: &mut events,
        port: 0,
    });
    let mut raw = ptr::null_mut();
    let error = unsafe {
        DNSServiceBrowse(
            &mut raw,
            PEER_FLAGS | 1,
            0,
            c"_airdrop._tcp".as_ptr(),
            c"local.".as_ptr(),
            found,
            (&mut *ctx as *mut Context).cast(),
        )
    };
    if error != 0 {
        if !raw.is_null() {
            unsafe {
                DNSServiceRefDeallocate(raw);
            }
        }
        publish(Update::Failure(format!(
            "AirDrop network discovery failed ({error})."
        )));
        return;
    }
    let browser = OwnedService { raw, _context: ctx };
    let mut resolutions = HashMap::<String, OwnedService>::new();
    let mut addresses = HashMap::<String, OwnedService>::new();
    let mut identifying = HashMap::<String, Arc<AtomicBool>>::new();
    while !cancel.load(Ordering::Relaxed) {
        let services: Vec<Service> = std::iter::once(browser.raw)
            .chain(resolutions.values().map(|s| s.raw))
            .chain(addresses.values().map(|s| s.raw))
            .collect();
        let mut fds: Vec<libc::pollfd> = services
            .iter()
            .map(|s| libc::pollfd {
                fd: unsafe { DNSServiceRefSockFD(*s) },
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 200) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        for (fd, service) in fds.iter().zip(services) {
            if fd.revents & libc::POLLIN != 0 {
                let error = unsafe { DNSServiceProcessResult(service) };
                if error != 0 {
                    events.push(Event::Failure(error));
                }
            }
        }
        for event in std::mem::take(&mut events) {
            match event {
                Event::Found {
                    id,
                    name,
                    kind,
                    domain,
                    interface,
                } => {
                    if resolutions.contains_key(&id) || resolutions.len() >= 64 {
                        continue;
                    }
                    let mut ctx = Box::new(Context {
                        id: id.clone(),
                        events: &mut events,
                        port: 0,
                    });
                    let mut raw = ptr::null_mut();
                    let error = unsafe {
                        DNSServiceResolve(
                            &mut raw,
                            PEER_FLAGS,
                            interface,
                            name.as_ptr(),
                            kind.as_ptr(),
                            domain.as_ptr(),
                            resolved,
                            (&mut *ctx as *mut Context).cast(),
                        )
                    };
                    if error == 0 {
                        resolutions.insert(id, OwnedService { raw, _context: ctx });
                    } else if !raw.is_null() {
                        unsafe {
                            DNSServiceRefDeallocate(raw);
                        }
                    }
                }
                Event::Removed(id) => {
                    resolutions.remove(&id);
                    addresses.remove(&id);
                    if let Some(stop) = identifying.remove(&id) {
                        stop.store(true, Ordering::Relaxed);
                    }
                    publish(Update::Removed(id));
                }
                Event::Resolved {
                    id,
                    host,
                    port,
                    interface,
                } => {
                    if addresses.contains_key(&id) || !resolutions.contains_key(&id) {
                        continue;
                    }
                    let mut ctx = Box::new(Context {
                        id: id.clone(),
                        events: &mut events,
                        port,
                    });
                    let mut raw = ptr::null_mut();
                    let error = unsafe {
                        DNSServiceGetAddrInfo(
                            &mut raw,
                            PEER_FLAGS,
                            interface,
                            2,
                            host.as_ptr(),
                            address,
                            (&mut *ctx as *mut Context).cast(),
                        )
                    };
                    if error == 0 {
                        addresses.insert(id, OwnedService { raw, _context: ctx });
                    } else if !raw.is_null() {
                        unsafe {
                            DNSServiceRefDeallocate(raw);
                        }
                    }
                }
                Event::Address { id, address } => {
                    if identifying.contains_key(&id) || !resolutions.contains_key(&id) {
                        continue;
                    }
                    let device = Device {
                        id: id.clone(),
                        name: "Nearby AirDrop device".into(),
                        detail: "Identifying…".into(),
                        endpoint: address,
                        ready: false,
                    };
                    publish(Update::Device(device.clone()));
                    let stop = Arc::new(AtomicBool::new(false));
                    identifying.insert(id, stop.clone());
                    let tx = tx.clone();
                    let waker = waker.clone();
                    std::thread::spawn(move || {
                        let mut device = device;
                        match network::identify(device.endpoint, &stop) {
                            Ok(name) => {
                                device.name = name;
                                device.detail = "AirDrop".into();
                                device.ready = true;
                            }
                            Err(_) => device.detail = "Couldn't identify. Refresh to retry.".into(),
                        }
                        if !stop.load(Ordering::Relaxed) {
                            let _ = tx.send(Update::Identified(device));
                            waker();
                        }
                    });
                }
                Event::Failure(code) => {
                    publish(Update::Failure(format!(
                        "AirDrop network discovery failed ({code})."
                    )));
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    }
    for stop in identifying.values() {
        stop.store(true, Ordering::Relaxed);
    }
}
