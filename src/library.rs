//! Background SMB work for the headset browser. Network calls never run on the
//! XR frame loop: requests go to worker threads and results come back on a
//! channel. Navigation and opening use one worker, playability probes another,
//! so a slow probe never delays browsing.

use crate::config::Server;
use crate::media::{Media, VideoDecoder};
use crate::playability::{self, Assessment, Platform};
use crate::readahead::ReadAhead;
use crate::smb::{Entry, SmbSession, SmbUrl};
use crate::vr::{self, Layout};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

pub type Path = Vec<String>;

pub enum Request {
    Shares {
        id: u64,
        server: usize,
    },
    List {
        id: u64,
        server: usize,
        share: String,
        path: Path,
    },
    Probe {
        generation: u64,
        server: usize,
        share: String,
        path: Path,
    },
    Open {
        id: u64,
        server: usize,
        share: String,
        path: Path,
    },
}

pub struct Opened {
    pub decoder: VideoDecoder,
    pub layout: Layout,
    pub assessment: Assessment,
    pub name: String,
}

pub enum Response {
    Shares {
        id: u64,
        result: Result<Vec<String>, String>,
    },
    List {
        id: u64,
        result: Result<Vec<Entry>, String>,
    },
    Probe {
        generation: u64,
        name: String,
        result: Result<Assessment, String>,
    },
    Opened {
        id: u64,
        result: Result<Box<Opened>, String>,
    },
}

pub struct Library {
    pub servers: Vec<Server>,
    main: mpsc::Sender<Request>,
    probes: mpsc::Sender<Request>,
    responses: mpsc::Receiver<Response>,
    /// Probes for other generations (folders left behind) are skipped.
    probe_generation: Arc<AtomicU64>,
}

type Sessions = Arc<Mutex<HashMap<usize, Arc<SmbSession>>>>;

fn session(
    servers: &[Server],
    sessions: &Sessions,
    index: usize,
) -> Result<Arc<SmbSession>, String> {
    if let Some(s) = sessions.lock().expect("sessions").get(&index) {
        return Ok(s.clone());
    }
    let server = servers.get(index).ok_or("Unknown server")?;
    let url: SmbUrl = server.url.parse().map_err(|e| format!("{e:#}"))?;
    let password = crate::config::password(&server.url)
        .map_err(|e| format!("{e:#}"))?
        .unwrap_or_default();
    let session = Arc::new(
        SmbSession::connect(url, password)
            .map_err(|e| format!("Can't connect to {}: {e:#}", server.name))?,
    );
    sessions
        .lock()
        .expect("sessions")
        .insert(index, session.clone());
    Ok(session)
}

/// Forgets a server connection after a failure so the next request reconnects;
/// a broken connection must never wedge browsing until the app restarts.
fn evict(sessions: &Sessions, index: usize) {
    if sessions.lock().expect("sessions").remove(&index).is_some() {
        eprintln!("Library: reconnecting to server {index} on next request");
    }
}

fn smb_path(path: &Path) -> String {
    path.join("\\")
}

fn open_media(
    session: &SmbSession,
    share: &str,
    path: &Path,
    read_ahead: ReadAhead,
) -> anyhow::Result<Media> {
    let reader = session.open_in(share, &smb_path(path), read_ahead)?;
    Media::open(path.last().map_or("", String::as_str), reader)
}

fn handle(request: Request, servers: &[Server], sessions: &Sessions, hw: Option<&str>) -> Response {
    let started = std::time::Instant::now();
    let (server, what) = match &request {
        Request::Shares { server, .. } => (*server, "shares"),
        Request::List { server, .. } => (*server, "list"),
        Request::Probe { server, .. } => (*server, "probe"),
        Request::Open { server, .. } => (*server, "open"),
    };
    let response = run(request, servers, sessions, hw);
    let failure = match &response {
        Response::Shares { result: Err(e), .. }
        | Response::List { result: Err(e), .. }
        | Response::Opened { result: Err(e), .. } => Some(e.clone()),
        // A bad file is not a connection problem; a stalled server is.
        Response::Probe { result: Err(e), .. } if e.contains("stopped sending") || e.contains("timed out") => Some(e.clone()),
        _ => None,
    };
    if let Some(e) = failure {
        eprintln!("Library: {what} failed after {:.1}s: {e}", started.elapsed().as_secs_f64());
        evict(sessions, server);
    } else if started.elapsed().as_secs_f64() > 2.0 {
        eprintln!("Library: {what} took {:.1}s", started.elapsed().as_secs_f64());
    }
    response
}

fn run(request: Request, servers: &[Server], sessions: &Sessions, hw: Option<&str>) -> Response {
    let err = |e: anyhow::Error| format!("{e:#}");
    match request {
        Request::Shares { id, server } => Response::Shares {
            id,
            result: session(servers, sessions, server).and_then(|s| s.shares().map_err(err)),
        },
        Request::List {
            id,
            server,
            share,
            path,
        } => Response::List {
            id,
            result: session(servers, sessions, server)
                .and_then(|s| s.list_in(&share, &smb_path(&path)).map_err(err)),
        },
        Request::Probe {
            generation,
            server,
            share,
            path,
        } => {
            // Header probes read little: small blocks, shallow read-ahead.
            let probe = ReadAhead {
                block_size: 256 * 1024,
                blocks_ahead: 4,
            };
            let result = session(servers, sessions, server).and_then(|s| {
                open_media(&s, &share, &path, probe)
                    .map(|m| playability::assess(Platform::current(), m.info().video.as_ref()))
                    .map_err(err)
            });
            Response::Probe {
                generation,
                name: path.last().cloned().unwrap_or_default(),
                result,
            }
        }
        Request::Open {
            id,
            server,
            share,
            path,
        } => {
            let result = session(servers, sessions, server).and_then(|s| {
                let media = open_media(&s, &share, &path, ReadAhead::default()).map_err(err)?;
                let name = path.last().cloned().unwrap_or_default();
                let video = media.info().video.clone();
                let assessment = playability::assess(Platform::current(), video.as_ref());
                let layout = vr::detect(&name, video.as_ref());
                let decoder = media.into_decoder(hw, true, "").map_err(err)?;
                Ok(Box::new(Opened {
                    decoder,
                    layout,
                    assessment,
                    name,
                }))
            });
            Response::Opened { id, result }
        }
    }
}

impl Library {
    /// `hw` is the preferred hardware backend for playback (see `media::default_hw_backend`).
    pub fn start(servers: Vec<Server>, hw: Option<&'static str>) -> Self {
        let sessions: Sessions = Default::default();
        let probe_generation = Arc::new(AtomicU64::new(0));
        let (response_tx, responses) = mpsc::channel();
        let mut senders = Vec::new();
        for name in ["library", "probe"] {
            let (tx, rx) = mpsc::channel::<Request>();
            let (servers, sessions, out) = (servers.clone(), sessions.clone(), response_tx.clone());
            let current = probe_generation.clone();
            std::thread::Builder::new()
                .name(name.into())
                .spawn(move || {
                    for request in rx {
                        if let Request::Probe { generation, .. } = &request
                            && *generation != current.load(Ordering::Relaxed)
                        {
                            continue;
                        }
                        if out.send(handle(request, &servers, &sessions, hw)).is_err() {
                            return;
                        }
                    }
                })
                .expect("spawn library worker");
            senders.push(tx);
        }
        let probes = senders.pop().expect("probe worker");
        let main = senders.pop().expect("main worker");
        Self {
            servers,
            main,
            probes,
            responses,
            probe_generation,
        }
    }

    /// Only probes tagged with this generation will run from now on.
    pub fn set_probe_generation(&self, generation: u64) {
        self.probe_generation.store(generation, Ordering::Relaxed);
    }

    pub fn send(&self, request: Request) {
        let worker = if matches!(request, Request::Probe { .. }) {
            &self.probes
        } else {
            &self.main
        };
        let _ = worker.send(request);
    }

    pub fn try_recv(&self) -> Option<Response> {
        self.responses.try_recv().ok()
    }
}
