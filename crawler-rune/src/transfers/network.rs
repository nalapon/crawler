//! AirDrop's experimental Everyone-mode HTTPS protocol. TLS uses Security.framework,
//! basic X.509 policy and only Apple root anchors.
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFType, TCFType},
    boolean::CFBoolean,
    data::{CFData, CFDataRef},
    dictionary::{CFDictionary, CFDictionaryRef},
    string::{CFString, CFStringRef},
};
use plist::{Dictionary, Value};
use security_framework::{
    identity::SecIdentity,
    policy::SecPolicy,
    secure_transport::{HandshakeError, SslConnectionType, SslContext, SslProtocolSide, SslStream},
    trust::SecTrust,
};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Write},
    net::{SocketAddr, SocketAddrV6, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;
use uuid::Uuid;

type Result<T> = std::result::Result<T, String>;
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecImportExportPassphrase: CFStringRef;
    static kSecImportItemIdentity: CFStringRef;
    fn SecPKCS12Import(data: CFDataRef, options: CFDictionaryRef, items: *mut CFArrayRef) -> i32;
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err("Transfer cancelled".into())
    } else {
        Ok(())
    }
}
fn macos15() -> bool {
    let mut buffer = [0u8; 64];
    let mut size = buffer.len();
    let status = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    status == 0
        && String::from_utf8_lossy(&buffer[..size.min(buffer.len())])
            .split('.')
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .is_some_and(|v| v >= 15)
}
fn run_command(command: &mut Command, input: Option<&[u8]>, cancel: &AtomicBool) -> Result<()> {
    check(cancel)?;
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::piped())
        .spawn()
        .map_err(err)?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(input) = input {
            if let Err(e) = stdin.write_all(input) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err(e));
            }
        }
    }
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Transfer cancelled".into());
        }
        match child.try_wait().map_err(err)? {
            Some(status) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err("Couldn't prepare the AirDrop transfer.".into())
                };
            }
            None => std::thread::sleep(Duration::from_millis(30)),
        }
    }
}
fn sender_identity(cancel: &AtomicBool) -> Result<SecIdentity> {
    if !macos15() {
        return Err("Custom AirDrop requires macOS 15 or later.".into());
    }
    static IDENTITY: OnceLock<Mutex<Option<SecIdentity>>> = OnceLock::new();
    let mut saved = IDENTITY
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| "Couldn't load the AirDrop identity")?;
    if let Some(identity) = &*saved {
        return Ok(identity.clone());
    }
    let directory = tempfile::Builder::new()
        .prefix("Crawler-AirDrop-Identity-")
        .tempdir()
        .map_err(err)?;
    let key = directory.path().join("sender-key.pem");
    let cert = directory.path().join("sender.pem");
    let bundle = directory.path().join("sender.p12");
    let passphrase = Uuid::new_v4().to_string();
    run_command(
        Command::new("/usr/bin/openssl")
            .args(["req", "-newkey", "rsa:2048", "-nodes", "-keyout"])
            .arg(&key)
            .args(["-x509", "-days", "365", "-out"])
            .arg(&cert)
            .args(["-subj", "/CN=Crawler"]),
        None,
        cancel,
    )?;
    run_command(
        Command::new("/usr/bin/openssl")
            .args(["pkcs12", "-export", "-inkey"])
            .arg(&key)
            .arg("-in")
            .arg(&cert)
            .arg("-out")
            .arg(&bundle)
            .args([
                "-keypbe",
                "PBE-SHA1-3DES",
                "-certpbe",
                "PBE-SHA1-3DES",
                "-macalg",
                "sha1",
                "-passout",
                "stdin",
            ]),
        Some(format!("{passphrase}\n").as_bytes()),
        cancel,
    )?;
    let data = CFData::from_buffer(&fs::read(bundle).map_err(err)?);
    // This option was added in macOS 15. Resolve it only after the availability check,
    // rather than making launch on older systems depend on a new framework symbol.
    let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"kSecImportToMemoryOnly".as_ptr()) };
    if symbol.is_null() {
        return Err("In-memory AirDrop identities are unavailable.".into());
    }
    let memory_key = unsafe { CFString::wrap_under_get_rule(*(symbol as *const CFStringRef)) };
    let password_key = unsafe { CFString::wrap_under_get_rule(kSecImportExportPassphrase) };
    let options = CFDictionary::from_CFType_pairs(&[
        (memory_key, CFBoolean::true_value().as_CFType()),
        (password_key, CFString::new(&passphrase).as_CFType()),
    ]);
    let mut imported = ptr::null();
    let status = unsafe {
        SecPKCS12Import(
            data.as_concrete_TypeRef(),
            options.as_concrete_TypeRef(),
            &mut imported,
        )
    };
    if status != 0 || imported.is_null() {
        return Err(format!(
            "Couldn't load the AirDrop sender certificate ({status})."
        ));
    }
    let items =
        unsafe { CFArray::<CFDictionary<CFString, CFType>>::wrap_under_create_rule(imported) };
    let identity_key = unsafe { CFString::wrap_under_get_rule(kSecImportItemIdentity) };
    let identity = items
        .iter()
        .next()
        .and_then(|item| {
            item.find(&identity_key)
                .and_then(|value| value.downcast::<SecIdentity>())
        })
        .ok_or("The AirDrop identity has an invalid type.")?;
    *saved = Some(identity.clone());
    Ok(identity)
}

struct Transport<'a> {
    tcp: TcpStream,
    cancel: &'a AtomicBool,
    deadline: Instant,
}
impl Transport<'_> {
    fn remaining(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Transfer cancelled",
            ))
        } else if Instant::now() >= self.deadline {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The receiving device didn't respond.",
            ))
        } else {
            Ok(())
        }
    }
}
impl Read for Transport<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            self.remaining()?;
            match self.tcp.read(buffer) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) => {}
                result => return result,
            }
        }
    }
}
impl Write for Transport<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        loop {
            self.remaining()?;
            match self.tcp.write(buffer) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) => {}
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.remaining()?;
        self.tcp.flush()
    }
}
struct Connection<'a> {
    stream: SslStream<Transport<'a>>,
    buffered: Vec<u8>,
}
impl<'a> Connection<'a> {
    fn connect(endpoint: SocketAddrV6, cancel: &'a AtomicBool) -> Result<Self> {
        let identity = sender_identity(cancel)?;
        check(cancel)?;
        let tcp = TcpStream::connect_timeout(&SocketAddr::V6(endpoint), Duration::from_secs(8))
            .map_err(err)?;
        tcp.set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(err)?;
        tcp.set_write_timeout(Some(Duration::from_millis(200)))
            .map_err(err)?;
        let mut ctx =
            SslContext::new(SslProtocolSide::CLIENT, SslConnectionType::STREAM).map_err(err)?;
        ctx.set_certificate(&identity, &[]).map_err(err)?;
        ctx.set_break_on_server_auth(true).map_err(err)?;
        let transport = Transport {
            tcp,
            cancel,
            deadline: Instant::now() + Duration::from_secs(8),
        };
        let mut result = ctx.handshake(transport);
        let mut verified = false;
        loop {
            match result {
                Ok(stream) if verified => {
                    return Ok(Self {
                        stream,
                        buffered: Vec::new(),
                    });
                }
                Ok(_) => {
                    return Err(
                        "The device did not provide a verifiable AirDrop certificate.".into(),
                    );
                }
                Err(HandshakeError::Interrupted(mid))
                    if mid.server_auth_completed() && !verified =>
                {
                    let mut trust = mid
                        .context()
                        .peer_trust2()
                        .map_err(err)?
                        .ok_or("The device didn't provide a certificate.")?;
                    let roots: Vec<_> = SecTrust::copy_anchor_certificates()
                        .map_err(err)?
                        .into_iter()
                        .filter(|c| c.subject_summary().starts_with("Apple Root CA"))
                        .collect();
                    if roots.is_empty() {
                        return Err("Apple root certificates are unavailable.".into());
                    }
                    trust.set_policy(&SecPolicy::create_x509()).map_err(err)?;
                    trust.set_anchor_certificates(&roots).map_err(err)?;
                    trust
                        .set_trust_anchor_certificates_only(true)
                        .map_err(err)?;
                    trust
                        .evaluate_with_error()
                        .map_err(|_| "The device's AirDrop certificate could not be verified.")?;
                    verified = true;
                    result = mid.handshake();
                }
                Err(HandshakeError::Interrupted(mid)) => {
                    return Err(format!("AirDrop TLS failed: {}", mid.error()));
                }
                Err(HandshakeError::Failure(e)) => return Err(format!("AirDrop TLS failed: {e}")),
            }
        }
    }
    fn deadline(&mut self, seconds: u64) {
        self.stream.get_mut().deadline = Instant::now() + Duration::from_secs(seconds);
    }
    fn header(
        &mut self,
        path: &str,
        kind: &str,
        length: Option<u64>,
        transfer: Option<(&Uuid, u64)>,
    ) -> Result<()> {
        let length = match length {
            Some(n) => format!("Content-Length: {n}"),
            None => "Transfer-Encoding: chunked".into(),
        };
        let transfer = transfer
            .map(|(id, n)| {
                format!(
                    "TransferID: {}\r\nTotalBytes: {n}\r\n",
                    id.to_string().to_uppercase()
                )
            })
            .unwrap_or_default();
        self.stream.write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nUser-Agent: AirDrop/1.0\r\nAccept: */*\r\nContent-Type: {kind}\r\n{length}\r\n{transfer}Connection: keep-alive\r\n\r\n").as_bytes()).map_err(err)
    }
    fn post(&mut self, path: &str, body: &[u8]) -> Result<Vec<u8>> {
        let start = Instant::now();
        self.deadline(if path == "/Ask" { 120 } else { 12 });
        self.header(
            path,
            "application/octet-stream",
            Some(body.len() as u64),
            None,
        )?;
        self.stream.write_all(body).map_err(err)?;
        let response = self.response()?;
        eprintln!(
            "AirDrop {path} acknowledged in {} ms",
            start.elapsed().as_millis()
        );
        Ok(response)
    }
    fn receive(&mut self) -> Result<()> {
        let mut chunk = [0u8; 65536];
        let n = self.stream.read(&mut chunk).map_err(err)?;
        if n == 0 {
            return Err("The device closed the connection.".into());
        }
        self.buffered.extend_from_slice(&chunk[..n]);
        Ok(())
    }
    fn line(&mut self, limit: usize) -> Result<Vec<u8>> {
        loop {
            if let Some(end) = self.buffered.windows(2).position(|w| w == b"\r\n") {
                if end > limit {
                    return Err("AirDrop response headers are too large.".into());
                }
                let line = self.buffered.drain(..end + 2).collect::<Vec<_>>();
                return Ok(line[..end].to_vec());
            }
            if self.buffered.len() > limit {
                return Err("AirDrop response headers are too large.".into());
            }
            self.receive()?;
        }
    }
    fn response(&mut self) -> Result<Vec<u8>> {
        let status = self.line(32768)?;
        let status = String::from_utf8(status).map_err(err)?;
        let code = status
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or("Invalid AirDrop response.")?;
        if !(200..300).contains(&code) {
            return Err(if code == 406 {
                "The receiving device declined the transfer.".into()
            } else {
                format!("The device returned HTTP {code}.")
            });
        }
        let mut length = None;
        let mut chunked = false;
        let mut header_bytes = status.len();
        loop {
            let line = self.line(32768)?;
            header_bytes += line.len() + 2;
            if header_bytes > 32768 {
                return Err("AirDrop response headers are too large.".into());
            }
            if line.is_empty() {
                break;
            }
            let line = String::from_utf8(line).map_err(err)?;
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = Some(value.trim().parse::<usize>().map_err(err)?);
                }
                if name.eq_ignore_ascii_case("transfer-encoding")
                    && value.trim().eq_ignore_ascii_case("chunked")
                {
                    chunked = true;
                }
            }
        }
        let mut out = Vec::new();
        if chunked {
            loop {
                let line = String::from_utf8(self.line(32768)?).map_err(err)?;
                let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
                    .map_err(err)?;
                if size > 1_048_576usize.saturating_sub(out.len()) {
                    return Err("AirDrop response is too large.".into());
                }
                if size == 0 {
                    let mut trailers = 0;
                    loop {
                        let line = self.line(32768)?;
                        trailers += line.len() + 2;
                        if trailers > 32768 {
                            return Err("AirDrop response trailers are too large.".into());
                        }
                        if line.is_empty() {
                            return Ok(out);
                        }
                    }
                }
                while self.buffered.len() < size + 2 {
                    self.receive()?;
                }
                if &self.buffered[size..size + 2] != b"\r\n" {
                    return Err("Invalid AirDrop response chunk.".into());
                }
                out.extend(self.buffered.drain(..size));
                self.buffered.drain(..2);
            }
        }
        let count = length
            .filter(|n| *n <= 1_048_576)
            .ok_or("AirDrop response has no bounded body length.")?;
        while self.buffered.len() < count {
            self.receive()?;
        }
        Ok(self.buffered.drain(..count).collect())
    }
    fn upload(
        &mut self,
        prepared: &AirDropPreparedFiles,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<()> {
        let mut file = File::open(&prepared.archive).map_err(err)?;
        let total = file.metadata().map_err(err)?.len();
        let start = Instant::now();
        self.deadline(120);
        self.header(
            "/Upload",
            "application/x-cpio",
            None,
            Some((&prepared.transfer_id, total)),
        )?;
        let mut buffer = vec![0u8; 262144];
        let mut sent = 0;
        loop {
            let count = file.read(&mut buffer).map_err(err)?;
            if count == 0 {
                break;
            }
            // Each submitted chunk extends the transport deadline; no percentage is inferred.
            self.deadline(30);
            self.stream
                .write_all(format!("{count:x}\r\n").as_bytes())
                .map_err(err)?;
            self.stream.write_all(&buffer[..count]).map_err(err)?;
            self.stream.write_all(b"\r\n").map_err(err)?;
            sent += count as u64;
            progress(sent, total);
        }
        self.stream.write_all(b"0\r\n\r\n").map_err(err)?;
        eprintln!(
            "AirDrop upload submitted {sent} archive bytes in {} ms; awaiting receiver acknowledgment",
            start.elapsed().as_millis()
        );
        self.deadline(30);
        self.response()?;
        eprintln!(
            "AirDrop upload acknowledged in {} ms",
            start.elapsed().as_millis()
        );
        Ok(())
    }
}

fn dictionary(values: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Dictionary(
        values
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}
fn binary(value: &Value) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    value.to_writer_binary(&mut out).map_err(err)?;
    Ok(out)
}

/// The accepted Ask UUID and Upload UUID cannot come from different preparations.
pub struct AirDropPreparedFiles {
    _directory: TempDir,
    archive: PathBuf,
    ask: Vec<u8>,
    transfer_id: Uuid,
}
impl AirDropPreparedFiles {
    pub fn prepare(files: &[PathBuf], cancel: &AtomicBool) -> Result<Self> {
        check(cancel)?;
        if files.is_empty() {
            return Err("Choose files to send.".into());
        }
        let mut names = HashSet::new();
        let mut entries = Vec::new();
        for file in files {
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or("A file has no valid name.")?;
            if !names.insert(name) {
                return Err("Choose files with different names for this transfer.".into());
            }
            let metadata = fs::metadata(file).map_err(err)?;
            entries.push(dictionary([
                ("FileName", name.into()),
                ("FileBomPath", format!("./{name}").into()),
                ("FileIsDirectory", metadata.is_dir().into()),
                ("FileType", file_type(file, metadata.is_dir()).into()),
                ("ConvertMediaFormats", 0u32.into()),
            ]));
        }
        let transfer_id = Uuid::new_v4();
        let hostname = objc2_foundation::NSProcessInfo::processInfo()
            .hostName()
            .to_string();
        let ask = binary(&dictionary([
            ("SenderComputerName", hostname.into()),
            ("SenderModelName", "Mac".into()),
            ("SenderID", Uuid::new_v4().simple().to_string()[..12].into()),
            ("BundleID", "studio.crawler.native-prototype".into()),
            ("ConvertMediaFormats", false.into()),
            ("Files", Value::Array(entries)),
            (
                "TransferID",
                dictionary([("id", transfer_id.to_string().to_uppercase().into())]),
            ),
            (
                "TransferType",
                dictionary([("files", Value::Dictionary(Dictionary::new()))]),
            ),
        ]))?;
        let directory = tempfile::Builder::new()
            .prefix("Crawler-AirDrop-")
            .tempdir()
            .map_err(err)?;
        let archive = directory.path().join("files.cpio.gz");
        let mut command = Command::new("/usr/bin/tar");
        command.args(["--format=cpio", "-czf"]).arg(&archive);
        for file in files {
            command
                .arg("-C")
                .arg(file.parent().ok_or("A file has no parent folder.")?)
                .arg(Path::new(".").join(file.file_name().ok_or("A file has no name.")?));
        }
        run_command(&mut command, None, cancel)?;
        Ok(Self {
            _directory: directory,
            archive,
            ask,
            transfer_id,
        })
    }
}
fn file_type(path: &Path, directory: bool) -> String {
    if directory {
        return "public.folder".into();
    }
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let extension = objc2_foundation::NSString::from_str(extension);
    objc2_uniform_type_identifiers::UTType::typeWithFilenameExtension(&extension)
        .map(|t| t.identifier().to_string())
        .unwrap_or_else(|| "public.data".into())
}
pub fn identify(endpoint: SocketAddrV6, cancel: &AtomicBool) -> Result<String> {
    let mut connection = Connection::connect(endpoint, cancel)?;
    let response = connection.post("/Discover", &binary(&Value::Dictionary(Dictionary::new()))?)?;
    let response = Value::from_reader(std::io::Cursor::new(response)).map_err(err)?;
    response
        .as_dictionary()
        .and_then(|d| d.get("ReceiverComputerName"))
        .and_then(Value::as_string)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .ok_or("The device didn't provide an AirDrop name.".into())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferUpdate {
    Preparing,
    Connecting,
    Asking,
    Uploading { sent: u64, total: u64 },
    AwaitingAcknowledgment,
    Finished(std::result::Result<(), String>),
}
pub fn send(
    endpoint: SocketAddrV6,
    files: Vec<PathBuf>,
    cancel: &AtomicBool,
    mut publish: impl FnMut(TransferUpdate),
) {
    let mut stage = "Preparing files";
    let result = (|| {
        publish(TransferUpdate::Preparing);
        let prepared = AirDropPreparedFiles::prepare(&files, cancel)?;
        stage = "Connecting";
        publish(TransferUpdate::Connecting);
        let mut connection = Connection::connect(endpoint, cancel)?;
        stage = "Requesting permission";
        publish(TransferUpdate::Asking);
        connection.post("/Ask", &prepared.ask)?;
        stage = "Sending files";
        connection.upload(&prepared, |sent, total| {
            publish(TransferUpdate::Uploading { sent, total });
            if sent >= total {
                publish(TransferUpdate::AwaitingAcknowledgment);
            }
        })
    })();
    publish(TransferUpdate::Finished(result.map_err(|e: String| {
        if cancel.load(Ordering::Relaxed) {
            "Transfer cancelled".into()
        } else {
            format!("{stage}: {e}")
        }
    })));
}
