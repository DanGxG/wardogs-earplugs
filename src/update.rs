use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{w, PCWSTR, PWSTR},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, WAIT_OBJECT_0,
            WPARAM,
        },
        Networking::WinHttp::*,
        Security::{Cryptography::*, WinTrust::*},
        Storage::FileSystem::{
            MoveFileExW, ReplaceFileW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
            REPLACE_FILE_FLAGS,
        },
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        System::Threading::{
            CreateEventW, OpenEventW, OpenProcess, QueryFullProcessImageNameW, SetEvent,
            WaitForSingleObject, CREATE_NO_WINDOW, EVENT_MODIFY_STATE, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        },
        UI::WindowsAndMessaging::{
            FindWindowW, GetWindowLongPtrW, GetWindowThreadProcessId, PostMessageW, GWLP_USERDATA,
            WM_APP,
        },
    },
};

pub const MESSAGE: u32 = WM_APP + 2;
const REPOSITORY: &str = "https://github.com/DanGxG/wardogs-earplugs";
const MANIFEST_LIMIT: usize = 4096;
const EXE_LIMIT: usize = 8 * 1024 * 1024;
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Version([u32; 3]);

impl Version {
    fn parse(text: &str) -> Result<Self> {
        let mut numbers = [0; 3];
        let parts: Vec<_> = text.split('.').collect();
        if parts.len() != 3 {
            return Err("Invalid release version.".into());
        }
        for (value, part) in numbers.iter_mut().zip(parts) {
            if part.is_empty()
                || !part.bytes().all(|c| c.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
            {
                return Err("Invalid release version.".into());
            }
            *value = part.parse().map_err(|_| "Invalid release version.")?;
        }
        Ok(Self(numbers))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0[0], self.0[1], self.0[2])
    }
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    requires_authenticode: bool,
    filename: String,
    hash: [u8; 32],
}

fn decode_hash(text: &str) -> Result<[u8; 32]> {
    if text.len() != 64 || !text.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid SHA-256 manifest.".into());
    }
    let mut hash = [0; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(hash)
}

fn manifest(text: &[u8]) -> Result<Release> {
    if text.len() > MANIFEST_LIMIT {
        return Err("Release manifest is too large.".into());
    }
    let text = std::str::from_utf8(text).map_err(|e| e.to_string())?;
    let mut lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .peekable();
    if lines.next() != Some("# earplugs-release-v2") {
        return Err("Unrecognized release manifest format.".into());
    }
    let requires_authenticode = lines.peek() == Some(&"# authenticode-required");
    if requires_authenticode {
        lines.next();
    }
    let mut selected: Option<Release> = None;
    let mut filenames = HashSet::new();
    for line in lines {
        let (hash, filename) = line.split_once("  ").ok_or("Invalid SHA-256 manifest.")?;
        let hash = decode_hash(hash)?;
        if !filename.ends_with(".exe") {
            continue;
        }
        if !filenames.insert(filename) {
            return Err("Duplicate release executable.".into());
        }
        let name = filename
            .strip_prefix("wardogs-earplugs-v")
            .ok_or("Unexpected release filename.")?;
        let version = name
            .strip_suffix("-windows-x64.exe")
            .ok_or("Unexpected release filename.")?;
        let release = Release {
            version: Version::parse(version)?,
            requires_authenticode,
            filename: filename.into(),
            hash,
        };
        if selected.is_some() {
            return Err("Ambiguous release manifest.".into());
        }
        selected = Some(release);
    }
    selected.ok_or_else(|| "Release manifest has no Windows executable.".into())
}

fn approved_url(url: &str) -> Result<(&str, &str)> {
    if url.len() > 16384
        || !url.is_ascii()
        || url.bytes().any(|c| c <= b' ' || c == b'\\' || c == b'#')
    {
        return Err("Invalid update URL.".into());
    }
    let value = url
        .strip_prefix("https://")
        .ok_or("Updates require HTTPS.")?;
    let (host, path) = value.split_once('/').ok_or("Invalid update URL.")?;
    if !matches!(
        host,
        "github.com" | "release-assets.githubusercontent.com" | "objects.githubusercontent.com"
    ) {
        return Err("Update redirect left GitHub.".into());
    }
    Ok((host, &url[url.len() - path.len() - 1..]))
}

fn wide(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

struct Internet(*mut std::ffi::c_void);
impl Internet {
    fn checked(handle: *mut std::ffi::c_void) -> Result<Self> {
        if handle.is_null() {
            Err(windows::core::Error::from_win32().to_string())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Internet {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn http_get(
    url: &str,
    limit: usize,
    cancelled: &AtomicBool,
    mut sink: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    unsafe {
        let session = Internet::checked(WinHttpOpen(
            w!("WarDogsEarplugs updater"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            None,
            None,
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 3000, 3000, 3000, 5000).map_err(|e| e.to_string())?;
        let mut url = url.to_string();
        let deadline = Instant::now() + Duration::from_secs(60);
        for _ in 0..6 {
            if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
                return Err("Update request cancelled or timed out.".into());
            }
            let (host, path) = approved_url(&url)?;
            let host = wide(host);
            let path = wide(path);
            let connection =
                Internet::checked(WinHttpConnect(session.0, PCWSTR(host.as_ptr()), 443, 0))?;
            let request = Internet::checked(WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                PCWSTR(path.as_ptr()),
                None,
                None,
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ))?;
            let disabled = WINHTTP_DISABLE_REDIRECTS
                | WINHTTP_DISABLE_COOKIES
                | WINHTTP_DISABLE_AUTHENTICATION;
            WinHttpSetOption(
                Some(request.0),
                WINHTTP_OPTION_DISABLE_FEATURE,
                Some(&disabled.to_ne_bytes()),
            )
            .map_err(|e| e.to_string())?;
            WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|e| e.to_string())?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| e.to_string())?;
            let mut status = 0u32;
            let mut length = 4;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                None,
                Some((&mut status as *mut u32).cast()),
                &mut length,
                std::ptr::null_mut(),
            )
            .map_err(|e| e.to_string())?;
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                length = 0;
                let _ = WinHttpQueryHeaders(
                    request.0,
                    WINHTTP_QUERY_LOCATION,
                    None,
                    None,
                    &mut length,
                    std::ptr::null_mut(),
                );
                if length == 0 || length > 32768 || length % 2 != 0 {
                    return Err("Invalid GitHub redirect.".into());
                }
                let mut location = vec![0u16; length as usize / 2];
                WinHttpQueryHeaders(
                    request.0,
                    WINHTTP_QUERY_LOCATION,
                    None,
                    Some(location.as_mut_ptr().cast()),
                    &mut length,
                    std::ptr::null_mut(),
                )
                .map_err(|e| e.to_string())?;
                let end = location
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(location.len());
                url = String::from_utf16(&location[..end]).map_err(|e| e.to_string())?;
                continue;
            }
            if status != 200 {
                return Err(format!("GitHub returned HTTP {status}."));
            }
            let mut buffer = [0u8; 8192];
            let mut total = 0;
            loop {
                if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
                    return Err("Update request cancelled or timed out.".into());
                }
                let mut read = 0;
                WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut read,
                )
                .map_err(|e| e.to_string())?;
                if read == 0 {
                    return Ok(());
                }
                total += read as usize;
                if total > limit {
                    return Err("Update download exceeds its size limit.".into());
                }
                sink(&buffer[..read as usize])?;
            }
        }
        Err("Too many GitHub redirects.".into())
    }
}

struct Hash {
    algorithm: BCRYPT_ALG_HANDLE,
    hash: BCRYPT_HASH_HANDLE,
}
impl Hash {
    fn new() -> Result<Self> {
        let mut state = Self {
            algorithm: BCRYPT_ALG_HANDLE::default(),
            hash: BCRYPT_HASH_HANDLE::default(),
        };
        unsafe {
            BCryptOpenAlgorithmProvider(
                &mut state.algorithm,
                BCRYPT_SHA256_ALGORITHM,
                None,
                BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS::default(),
            )
            .ok()
            .map_err(|e| e.to_string())?;
            // Windows 10 allocates the small hash object when no buffer is supplied.
            BCryptCreateHash(state.algorithm, &mut state.hash, None, None, 0)
                .ok()
                .map_err(|e| e.to_string())?;
        }
        Ok(state)
    }
    fn add(&self, bytes: &[u8]) -> Result<()> {
        unsafe {
            BCryptHashData(self.hash, bytes, 0)
                .ok()
                .map_err(|e| e.to_string())
        }
    }
    fn finish(&self) -> Result<[u8; 32]> {
        let mut bytes = [0; 32];
        unsafe {
            BCryptFinishHash(self.hash, &mut bytes, 0)
                .ok()
                .map_err(|e| e.to_string())?;
        }
        Ok(bytes)
    }
}
impl Drop for Hash {
    fn drop(&mut self) {
        unsafe {
            if !self.hash.is_invalid() {
                let _ = BCryptDestroyHash(self.hash);
            }
            if !self.algorithm.is_invalid() {
                let _ = BCryptCloseAlgorithmProvider(self.algorithm, 0);
            }
        }
    }
}

fn file_hash(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > EXE_LIMIT as u64 {
        return Err("Update executable is too large.".into());
    }
    let state = Hash::new()?;
    let mut buffer = [0; 8192];
    let mut total = 0;
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            return state.finish();
        }
        total += read;
        if total > EXE_LIMIT {
            return Err("Update executable grew beyond its size limit.".into());
        }
        state.add(&buffer[..read])?;
    }
}

fn validate_exe(path: &Path, release: &Release) -> Result<()> {
    if file_hash(path)? != release.hash {
        return Err("La descarga no coincide con SHA-256; no se instalará.".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut header = [0; 64];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    let offset = u32::from_le_bytes(header[60..64].try_into().unwrap()) as u64;
    if &header[..2] != b"MZ" || offset < 64 || offset > EXE_LIMIT as u64 - 26 {
        return Err("Invalid Windows executable.".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut pe = [0; 26];
    file.read_exact(&mut pe).map_err(|e| e.to_string())?;
    if &pe[..4] != b"PE\0\0" || pe[4..6] != [0x64, 0x86] || pe[24..26] != [0x0b, 0x02] {
        return Err("Update is not a Windows x64 executable.".into());
    }
    if release.requires_authenticode {
        verify_signature(path)?;
    }
    Ok(())
}

fn verify_signature(path: &Path) -> Result<()> {
    let name = wide(path);
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(name.as_ptr()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    unsafe {
        let status = WinVerifyTrust(
            HWND(-1isize as *mut _),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        let _ = WinVerifyTrust(
            HWND(-1isize as *mut _),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        if status != 0 {
            return Err("La descarga no pasa la verificación Authenticode.".into());
        }
    }
    Ok(())
}

pub enum Event {
    Checked(Result<Option<Release>>),
    Downloaded(Result<Staged>),
    Notice(String),
}

pub struct Updater {
    send: mpsc::Sender<Event>,
    receive: mpsc::Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    window: usize,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Updater {
    pub fn start(window: HWND) -> Result<Self> {
        let (send, receive) = mpsc::channel();
        let mut updater = Self {
            send,
            receive,
            cancelled: Arc::new(AtomicBool::new(false)),
            window: window.0 as usize,
            worker: None,
        };
        updater.spawn(|cancelled| {
            #[cfg(test)]
            if let Some(result) = tests::fixture_offer() {
                return Event::Checked(result.map(Some));
            }
            cleanup_previous_helper();
            if let Some(notice) = startup_notice() {
                return Event::Notice(notice);
            }
            let mut bytes = Vec::new();
            let result = http_get(
                &format!("{REPOSITORY}/releases/latest/download/SHA256SUMS.txt"),
                MANIFEST_LIMIT,
                cancelled,
                |chunk| {
                    bytes.extend_from_slice(chunk);
                    Ok(())
                },
            )
            .and_then(|_| manifest(&bytes))
            .and_then(|release| {
                Ok(
                    (release.version > Version::parse(env!("CARGO_PKG_VERSION"))?)
                        .then_some(release),
                )
            });
            Event::Checked(result)
        })?;
        Ok(updater)
    }
    fn spawn(&mut self, task: impl FnOnce(&AtomicBool) -> Event + Send + 'static) -> Result<()> {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let send = self.send.clone();
        let cancelled = Arc::clone(&self.cancelled);
        let window = self.window;
        self.worker = Some(
            std::thread::Builder::new()
                .name("earplugs-update".into())
                .spawn(move || {
                    let event = task(&cancelled);
                    if !cancelled.load(Ordering::Relaxed) && send.send(event).is_ok() {
                        unsafe {
                            let _ = PostMessageW(
                                Some(HWND(window as *mut _)),
                                MESSAGE,
                                WPARAM(0),
                                LPARAM(0),
                            );
                        }
                    }
                })
                .map_err(|e| e.to_string())?,
        );
        Ok(())
    }
    pub fn receive(&mut self) -> Option<Event> {
        let event = self.receive.try_recv().ok()?;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        Some(event)
    }
    pub fn download(&mut self, release: Release) -> Result<()> {
        self.spawn(move |cancelled| Event::Downloaded(stage(release, cancelled)))
    }
}
impl Drop for Updater {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        // Normal Exit must let the worker close its file before removing staging.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct Staged {
    path: PathBuf,
    release: Release,
    handed_off: bool,
}
impl Drop for Staged {
    fn drop(&mut self) {
        if !self.handed_off {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn stage(release: Release, cancelled: &AtomicBool) -> Result<Staged> {
    let target = std::env::current_exe().map_err(|e| e.to_string())?;
    if fs::metadata(&target)
        .map_err(|e| e.to_string())?
        .permissions()
        .readonly()
    {
        return Err(
            "El EXE está protegido contra escritura. Actualiza la copia manualmente.".into(),
        );
    }
    let parent = target.parent().ok_or("Missing application directory.")?;
    let (path, mut file) = loop {
        let path = parent.join(format!(
            ".wardogs-earplugs-update-{}-{}.exe",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (path, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("No se puede escribir en la carpeta del EXE: {e}. Puedes descargar la actualización manualmente.")),
        }
    };
    let staged = Staged {
        path,
        release,
        handed_off: false,
    };
    let result = (|| {
        let url = format!(
            "{REPOSITORY}/releases/download/v{}/{}",
            staged.release.version, staged.release.filename
        );
        #[cfg(test)]
        if let Some(source) = std::env::var_os("EARPLUGS_TEST_UPDATE_SOURCE") {
            std::io::copy(
                &mut File::open(source).map_err(|e| e.to_string())?,
                &mut file,
            )
            .map_err(|e| e.to_string())?;
        } else {
            http_get(&url, EXE_LIMIT, cancelled, |chunk| {
                file.write_all(chunk).map_err(|e| e.to_string())
            })?;
        }
        #[cfg(not(test))]
        http_get(&url, EXE_LIMIT, cancelled, |chunk| {
            file.write_all(chunk).map_err(|e| e.to_string())
        })?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        validate_exe(&staged.path, &staged.release)
    })();
    result?;
    Ok(staged)
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

impl Staged {
    pub fn handoff(&mut self, background: bool) -> Result<()> {
        validate_exe(&self.path, &self.release)?;
        let target = std::env::current_exe().map_err(|e| e.to_string())?;
        let event_name = format!(
            "Local\\WarDogsEarplugsUpdate-{}",
            self.path.file_stem().unwrap().to_string_lossy()
        );
        let event_wide = wide(&event_name);
        let event = unsafe {
            let handle = CreateEventW(None, true, false, PCWSTR(event_wide.as_ptr()))
                .map_err(|e| e.to_string())?;
            let exists = GetLastError() == ERROR_ALREADY_EXISTS;
            let handle = Handle(handle);
            if exists {
                return Err("An update handoff is already running.".into());
            }
            handle
        };
        let checksum: String = self
            .release
            .hash
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut command = Command::new(&self.path);
        command
            .creation_flags(CREATE_NO_WINDOW.0)
            .arg("--finish-update")
            .arg(std::process::id().to_string())
            .arg(target)
            .arg(self.release.version.to_string())
            .arg(checksum)
            .arg(&event_name);
        if background {
            command.arg("--background");
        }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        if unsafe { WaitForSingleObject(event.0, 10000) } != WAIT_OBJECT_0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(
                "No se pudo iniciar la actualización; la versión actual sigue abierta.".into(),
            );
        }
        self.handed_off = true;
        Ok(())
    }
}

fn own_process(pid: u32, expected: &Path) -> Result<Handle> {
    if pid == 0 || pid == std::process::id() {
        return Err("Invalid updater process.".into());
    }
    unsafe {
        // Only the Earplugs parent/helper: never a game process or its memory.
        let process = Handle(
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
            .map_err(|e| e.to_string())?,
        );
        let mut name = vec![0u16; 32768];
        let mut length = name.len() as u32;
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(name.as_mut_ptr()),
            &mut length,
        )
        .map_err(|e| e.to_string())?;
        let actual =
            PathBuf::from(String::from_utf16(&name[..length as usize]).map_err(|e| e.to_string())?);
        if fs::canonicalize(actual).map_err(|e| e.to_string())?
            != fs::canonicalize(expected).map_err(|e| e.to_string())?
        {
            return Err("Updater parent path does not match Earplugs.".into());
        }
        Ok(process)
    }
}

pub fn helper_mode() -> Option<Result<()>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    (arguments
        .first()
        .is_some_and(|arg| arg == "--finish-update"))
    .then(|| finish_update(&arguments))
}

fn finish_update(arguments: &[std::ffi::OsString]) -> Result<()> {
    if !(arguments.len() == 6 || (arguments.len() == 7 && arguments[6] == "--background")) {
        return Err("Invalid update arguments.".into());
    }
    let pid = arguments[1]
        .to_str()
        .ok_or("Invalid process ID.")?
        .parse()
        .map_err(|_| "Invalid process ID.")?;
    let target = PathBuf::from(&arguments[2]);
    let source = std::env::current_exe().map_err(|e| e.to_string())?;
    if source.parent() != target.parent()
        || !source.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with(".wardogs-earplugs-update-")
        })
        || target
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("exe"))
        || source == target
    {
        return Err("Invalid updater paths.".into());
    }
    let version = Version::parse(arguments[3].to_str().ok_or("Invalid version.")?)?;
    if version != Version::parse(env!("CARGO_PKG_VERSION"))? {
        return Err("Downloaded executable version does not match its release.".into());
    }
    if file_hash(&source)? != decode_hash(arguments[4].to_str().ok_or("Invalid checksum.")?)? {
        return Err("Downloaded helper checksum changed.".into());
    }
    let expected_event = format!(
        "Local\\WarDogsEarplugsUpdate-{}",
        source.file_stem().unwrap().to_string_lossy()
    );
    if arguments[5] != expected_event.as_str() {
        return Err("Invalid update readiness event.".into());
    }
    verify_helper_parent(pid)?;
    let parent = own_process(pid, &target)?;
    let incoming = prepare_incoming(
        &source,
        &decode_hash(arguments[4].to_str().ok_or("Invalid checksum.")?)?,
    )?;
    if source.with_extension("backup").exists() {
        return Err("An earlier update backup exists; update cancelled.".into());
    }
    let event_name = wide(&arguments[5]);
    let ready = unsafe {
        Handle(
            OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(event_name.as_ptr()))
                .map_err(|e| e.to_string())?,
        )
    };
    unsafe {
        SetEvent(ready.0).map_err(|e| e.to_string())?;
    }
    if unsafe { WaitForSingleObject(parent.0, 30000) } != WAIT_OBJECT_0 {
        return Err("Earplugs did not exit; update cancelled.".into());
    }
    replace_and_restart(&source, &target, &incoming.0, arguments.len() == 7)
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn prepare_incoming(source: &Path, expected: &[u8; 32]) -> Result<Temporary> {
    let path = source.with_extension("incoming");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let temporary = Temporary(path);
    let result: Result<()> = (|| {
        std::io::copy(
            &mut File::open(source).map_err(|e| e.to_string())?,
            &mut output,
        )
        .map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        if &file_hash(&temporary.0)? != expected {
            return Err("Prepared update checksum changed.".into());
        }
        Ok(())
    })();
    result?;
    Ok(temporary)
}

fn replace_and_restart(
    source: &Path,
    target: &Path,
    incoming: &Path,
    background: bool,
) -> Result<()> {
    let mut recovered = false;
    let result = replace_with(source, target, incoming, |new| {
        let notice = (!new).then_some("La actualización no pudo arrancar. Se ha recuperado la versión anterior y se han conservado tus ajustes.");
        launch_ready(target, background, Some(source), notice)?;
        recovered = !new;
        Ok(())
    });
    if recovered {
        Ok(())
    } else {
        result
    }
}

fn replace_with(
    source: &Path,
    target: &Path,
    incoming: &Path,
    mut launch: impl FnMut(bool) -> Result<()>,
) -> Result<()> {
    let backup = source.with_extension("backup");
    if backup.exists() {
        return Err("An earlier update backup exists; update cancelled.".into());
    }
    let result: Result<()> = (|| {
        let target_wide = wide(target);
        let incoming_wide = wide(incoming);
        let backup_wide = wide(&backup);
        unsafe {
            ReplaceFileW(
                PCWSTR(target_wide.as_ptr()),
                PCWSTR(incoming_wide.as_ptr()),
                PCWSTR(backup_wide.as_ptr()),
                REPLACE_FILE_FLAGS::default(),
                None,
                None,
            )
            .map_err(|e| e.to_string())?;
        }
        launch(true)?;
        let _ = fs::remove_file(&backup);
        Ok(())
    })();
    if let Err(error) = result {
        recover_backup(&backup, target).map_err(|e| {
            format!(
                "{error}. No se pudo recuperar el EXE anterior: {e}. Copia de seguridad: {}",
                backup.display()
            )
        })?;
        launch(false).map_err(|e| format!("No se pudo actualizar: {error}. EXE anterior conservado en {}. No se pudo reabrir: {e}", target.display()))?;
        return Err(format!(
            "No se pudo actualizar: {error}. Se conservó la versión anterior."
        ));
    }
    Ok(())
}

fn recover_backup(backup: &Path, target: &Path) -> Result<()> {
    if backup.exists() {
        let from = wide(backup);
        let to = wide(target);
        unsafe {
            MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if !target.exists() {
        return Err("Neither the target nor its previous copy is available.".into());
    }
    Ok(())
}

fn launch_ready(
    target: &Path,
    background: bool,
    cleanup: Option<&Path>,
    notice: Option<&str>,
) -> Result<()> {
    let ready = unsafe {
        Handle(
            CreateEventW(None, true, false, w!("Local\\WarDogsEarplugsReady"))
                .map_err(|e| e.to_string())?,
        )
    };
    unsafe {
        windows::Win32::System::Threading::ResetEvent(ready.0).map_err(|e| e.to_string())?;
    }
    let mut command = Command::new(target);
    command.creation_flags(CREATE_NO_WINDOW.0);
    if background {
        command.arg("--background");
    }
    if let Some(path) = cleanup {
        command
            .arg("--cleanup-update")
            .arg(std::process::id().to_string())
            .arg(path);
    }
    if let Some(notice) = notice {
        command.arg("--update-error").arg(notice);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let initialized = unsafe {
        WaitForSingleObject(ready.0, 10000) == WAIT_OBJECT_0
            && FindWindowW(w!("WarDogsEarplugsWindow"), None).is_ok_and(|window| {
                let mut pid = 0;
                GetWindowThreadProcessId(window, Some(&mut pid));
                pid == child.id() && GetWindowLongPtrW(window, GWLP_USERDATA) != 0
            })
    };
    if !initialized {
        let _ = child.kill();
        let _ = child.wait();
        return Err("La nueva copia no terminó de arrancar.".into());
    }
    Ok(())
}

fn verify_helper_parent(expected: u32) -> Result<()> {
    unsafe {
        let window = FindWindowW(w!("WarDogsEarplugsWindow"), None)
            .map_err(|_| "The updater was not started by Earplugs.")?;
        let mut window_pid = 0;
        GetWindowThreadProcessId(window, Some(&mut window_pid));
        if window_pid != expected || GetWindowLongPtrW(window, GWLP_USERDATA) == 0 {
            return Err("The updater parent is not an initialized Earplugs window.".into());
        }
        let snapshot =
            Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| e.to_string())?);
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut next = Process32FirstW(snapshot.0, &mut entry);
        while next.is_ok() {
            if entry.th32ProcessID == std::process::id() {
                return if entry.th32ParentProcessID == expected {
                    Ok(())
                } else {
                    Err("The updater was not launched by the supplied parent.".into())
                };
            }
            next = Process32NextW(snapshot.0, &mut entry);
        }
        Err("Cannot identify the updater's parent.".into())
    }
}

fn startup_notice() -> Option<String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let index = args.iter().position(|arg| arg == "--update-error")?;
    args.get(index + 1)
        .map(|text| text.to_string_lossy().chars().take(1024).collect())
}

fn cleanup_previous_helper() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(index) = args.iter().position(|arg| arg == "--cleanup-update") else {
        return;
    };
    let Some(pid) = args
        .get(index + 1)
        .and_then(|p| p.to_str())
        .and_then(|p| p.parse::<u32>().ok())
    else {
        return;
    };
    let Some(path) = args.get(index + 2).map(PathBuf::from) else {
        return;
    };
    let Ok(target) = std::env::current_exe() else {
        return;
    };
    if path.parent() != target.parent()
        || path == target
        || !path.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with(".wardogs-earplugs-update-")
                && name.to_string_lossy().ends_with(".exe")
        })
    {
        return;
    }
    if let Ok(process) = own_process(pid, &path) {
        if unsafe { WaitForSingleObject(process.0, 30000) } != WAIT_OBJECT_0 {
            return;
        }
    } else if path.with_extension("backup").exists() {
        return;
    }
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn fixture_offer() -> Option<Result<Release>> {
        std::env::var_os("EARPLUGS_TEST_UPDATE_SOURCE").map(|path| {
            let mut hash = file_hash(Path::new(&path))?;
            if std::env::var_os("EARPLUGS_TEST_BAD_UPDATE_HASH").is_some() {
                hash[0] ^= 0xff;
            }
            Ok(Release {
                version: Version::parse(env!("CARGO_PKG_VERSION"))?,
                requires_authenticode: false,
                filename: format!(
                    "wardogs-earplugs-v{}-windows-x64.exe",
                    env!("CARGO_PKG_VERSION")
                ),
                hash,
            })
        })
    }

    #[test]
    #[ignore = "Child-only GUI fixture for scripts/test-updater.ps1"]
    fn updater_ui_fixture() {
        if std::env::var_os("EARPLUGS_TEST_UPDATE_SOURCE").is_some() {
            crate::ui::run().unwrap();
        }
    }

    struct Files {
        directory: PathBuf,
        target: PathBuf,
        source: PathBuf,
    }
    impl Files {
        fn new() -> Self {
            let directory = loop {
                let path = std::env::temp_dir().join(format!(
                    "earplugs-updater-{}-{}",
                    std::process::id(),
                    NEXT_FILE.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => break path,
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("{e}"),
                }
            };
            let files = Self {
                target: directory.join("Earplugs con espacios.exe"),
                source: directory.join(".wardogs-earplugs-update-test.exe"),
                directory,
            };
            fs::write(&files.target, b"original executable").unwrap();
            fs::write(&files.source, b"new executable").unwrap();
            files
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            for path in [
                &self.target,
                &self.source,
                &self.source.with_extension("incoming"),
                &self.source.with_extension("backup"),
            ] {
                let _ = fs::remove_file(path);
            }
            let _ = fs::remove_dir(&self.directory);
        }
    }

    #[test]
    fn replacement_rolls_back_failed_initialization() {
        let files = Files::new();
        let incoming = prepare_incoming(&files.source, &file_hash(&files.source).unwrap()).unwrap();
        let mut launches = Vec::new();
        let result = replace_with(&files.source, &files.target, &incoming.0, |new| {
            launches.push(new);
            if new {
                assert_eq!(fs::read(&files.target).unwrap(), b"new executable");
                Err("Startup fixture failed".into())
            } else {
                assert_eq!(fs::read(&files.target).unwrap(), b"original executable");
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(launches, [true, false]);
        assert_eq!(fs::read(&files.target).unwrap(), b"original executable");
    }

    #[test]
    fn locked_target_keeps_original_and_success_requires_readiness() {
        use std::os::windows::fs::OpenOptionsExt;
        let files = Files::new();
        let incoming = prepare_incoming(&files.source, &file_hash(&files.source).unwrap()).unwrap();
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&files.target)
            .unwrap();
        let mut launches = Vec::new();
        assert!(
            replace_with(&files.source, &files.target, &incoming.0, |new| {
                launches.push(new);
                Ok(())
            })
            .is_err()
        );
        assert_eq!(launches, [false]);
        assert_eq!(fs::read(&files.target).unwrap(), b"original executable");
        drop(locked);
        replace_with(&files.source, &files.target, &incoming.0, |new| {
            assert!(new);
            assert_eq!(
                fs::read(files.source.with_extension("backup")).unwrap(),
                b"original executable"
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&files.target).unwrap(), b"new executable");
        assert!(!files.source.with_extension("backup").exists());
    }

    #[test]
    fn partial_replacement_recovers_missing_target_from_backup() {
        let files = Files::new();
        let backup = files.source.with_extension("backup");
        fs::rename(&files.target, &backup).unwrap();
        recover_backup(&backup, &files.target).unwrap();
        assert_eq!(fs::read(&files.target).unwrap(), b"original executable");
        assert!(!backup.exists());
    }

    #[test]
    fn corrupt_payload_and_failed_preparation_never_replace_target() {
        let files = Files::new();
        let hash = file_hash(&files.source).unwrap();
        let release = Release {
            version: Version::parse("0.1.4").unwrap(),
            requires_authenticode: false,
            filename: "unused".into(),
            hash,
        };
        assert!(validate_exe(&files.source, &release).is_err());
        assert!(prepare_incoming(&files.source, &[0; 32]).is_err());
        assert!(!files.source.with_extension("incoming").exists());
        assert_eq!(fs::read(&files.target).unwrap(), b"original executable");
        assert!(decode_hash(&"g".repeat(64)).is_err());
    }

    #[test]
    fn versions_are_numeric_and_stable_only() {
        assert!(Version::parse("0.1.10").unwrap() > Version::parse("0.1.9").unwrap());
        for bad in [
            "v1.2.3",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.3-beta",
            "1.2.3/",
            "1.2.4294967296",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn manifest_selects_expected_exe_and_rejects_ambiguity() {
        let hash = "ab".repeat(32);
        let checksum = format!("# earplugs-release-v2\r\n{hash}  wardogs-earplugs-v0.2.0-windows-x64.exe\r\n{hash}  wardogs-earplugs-v0.2.0-windows-x64.zip\r\n");
        let found = manifest(checksum.as_bytes()).unwrap();
        assert_eq!(found.version, Version::parse("0.2.0").unwrap());
        assert!(!found.requires_authenticode);
        let signed = checksum.replace(
            "# earplugs-release-v2\r\n",
            "# earplugs-release-v2\r\n# authenticode-required\r\n",
        );
        assert!(manifest(signed.as_bytes()).unwrap().requires_authenticode);
        for bad in [
            format!("{checksum}{hash}  wardogs-earplugs-v0.2.0-windows-x64.exe"),
            format!("{checksum}{hash}  wardogs-earplugs-v0.3.0-windows-x64.exe"),
            format!("# earplugs-release-v2\n{hash}  ../evil.exe"),
            checksum.replace("# earplugs-release-v2", "# unknown-format"),
            checksum.replace("# earplugs-release-v2\r\n", ""),
            format!("# earplugs-release-v2\n# unknown-policy\n{hash}  wardogs-earplugs-v0.2.0-windows-x64.exe"),
            "broken".into(),
        ] {
            assert!(manifest(bad.as_bytes()).is_err());
        }
        assert!(manifest(&vec![b' '; MANIFEST_LIMIT + 1]).is_err());
    }
    #[test]
    fn update_urls_cannot_leave_https_github() {
        assert_eq!(
            approved_url("https://github.com/a/b").unwrap(),
            ("github.com", "/a/b")
        );
        assert!(approved_url("https://release-assets.githubusercontent.com/a?b=2").is_ok());
        for url in [
            "http://github.com/a",
            "https://github.com.evil/a",
            "https://github.com@evil/a",
            "https://evil/a",
            "https://github.com:443/a",
            "https://github.com/a\r\nb",
            "https://github.com/a#fragment",
            "https://github.com\\evil/a",
        ] {
            assert!(approved_url(url).is_err(), "{url}");
        }
    }
    #[test]
    fn native_sha256_matches_known_digest() {
        let hash = Hash::new().unwrap();
        hash.add(b"a").unwrap();
        hash.add(b"bc").unwrap();
        assert_eq!(
            hash.finish().unwrap(),
            decode_hash("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
                .unwrap()
        );
    }

    #[test]
    fn normal_exit_joins_worker_and_removes_its_staging_file() {
        let files = Files::new();
        let path = files.source.clone();
        let (send, receive) = mpsc::channel();
        let (ready, started) = mpsc::channel();
        let mut updater = Updater {
            send,
            receive,
            cancelled: Arc::new(AtomicBool::new(false)),
            window: 0,
            worker: None,
        };
        let source = path.clone();
        updater
            .spawn(move |cancelled| {
                let staged = Staged {
                    path: source,
                    release: Release {
                        version: Version::parse("0.1.4").unwrap(),
                        requires_authenticode: false,
                        filename: "test".into(),
                        hash: [0; 32],
                    },
                    handed_off: false,
                };
                ready.send(()).unwrap();
                while !cancelled.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Event::Downloaded(Ok(staged))
            })
            .unwrap();
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(updater);
        assert!(
            !path.exists(),
            "Normal Exit returned before staging cleanup completed"
        );
    }
    #[test]
    #[ignore = "Uses the public GitHub release over native Windows HTTPS"]
    fn real_github_manifest_and_verified_download() {
        let cancelled = AtomicBool::new(false);
        let mut bytes = Vec::new();
        http_get(
            &format!("{REPOSITORY}/releases/latest/download/SHA256SUMS.txt"),
            MANIFEST_LIMIT,
            &cancelled,
            |chunk| {
                bytes.extend_from_slice(chunk);
                Ok(())
            },
        )
        .unwrap();
        let release = manifest(&bytes).unwrap();
        let staged = stage(release.clone(), &cancelled).unwrap();
        validate_exe(&staged.path, &release).unwrap();
        let mut corrupt = release.clone();
        corrupt.hash[0] ^= 0xff;
        assert!(validate_exe(&staged.path, &corrupt).is_err());
        if !release.requires_authenticode {
            let mut advertised_signed = release.clone();
            advertised_signed.requires_authenticode = true;
            assert!(validate_exe(&staged.path, &advertised_signed).is_err());
        }
        let path = staged.path.clone();
        drop(staged);
        assert!(!path.exists());
    }
}
